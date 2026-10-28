#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <stdio.h>
#include <unistd.h>
#include <sys/socket.h>
#include <stdatomic.h>
#include <pthread.h>
static _Atomic uintptr_t journal_thread;
int pthread_join(pthread_t thread,void **result) {
    static int (*original)(pthread_t,void **);
    if(!original) original=dlsym(RTLD_NEXT,"pthread_join");
    int joined=original(thread,result);
    return getenv("S412_JOIN_FAIL") && (uintptr_t)thread==atomic_load(&journal_thread) && !joined ? EINVAL : joined;
}
static _Atomic int gated;
ssize_t write(int fd,const void *data,size_t size) {
    static ssize_t (*original)(int,const void *,size_t);
    if(!original) original=dlsym(RTLD_NEXT,"write");
    if(getenv("S412_REPORT_FAIL") && fd>2 && memmem(data,size,"command-reader-ui:",18)) { errno=EIO; return -1; }
    return original(fd,data,size);
}
static int journal(int fd) {
    char descriptor[64], path[4096];
    snprintf(descriptor,sizeof(descriptor),"/proc/self/fd/%d",fd);
    ssize_t n=readlink(descriptor,path,sizeof(path)-1);
    if(n<0) return 0;
    path[n]=0;
    return n>=16 && !strcmp(path+n-16,"/.reader-journal");
}
static void gate(void) {
    if(!atomic_exchange(&gated,1)) {
        char byte='1';
        write(atoi(getenv("S412_READY_FD")),&byte,1);
        read(atoi(getenv("S412_RELEASE_FD")),&byte,1);
    }
}
ssize_t send(int fd,const void *data,size_t size,int flags) {
    static ssize_t (*original)(int,const void *,size_t,int);
    if(!original) original=dlsym(RTLD_NEXT,"send");
    if(getenv("S412_PARTIAL") && size<=2048 && size>=6 && !memcmp(data,"data: ",6)) {
        char copy[2049]; memcpy(copy,data,size); copy[size]=0;
        char *child=strstr(copy,"\"child\":0");
        if(child) { child[8]='9'; return original(fd,copy,size,flags); }
    }
    return original(fd,data,size,flags);
}
int fsync(int fd) {
    static int (*original)(int);
    if(!original) original=dlsym(RTLD_NEXT,"fsync");
    const char *selected=getenv("S412_STORAGE_TAG");
    if(selected && journal(fd)) {
        char descriptor[64]; snprintf(descriptor,sizeof(descriptor),"/proc/self/fd/%d",fd);
        int reader=open(descriptor,O_RDONLY|O_CLOEXEC);
        unsigned char bytes[32768]; ssize_t n=reader<0?-1:pread(reader,bytes,sizeof(bytes),0);
        if(reader>=0) close(reader);
        size_t offset=0; int tag=-1;
        while(n>=7 && offset+7<=(size_t)n) {
            uint32_t length; memcpy(&length,bytes+offset,4);
            if(length<3 || length>4092 || offset+4+length>(size_t)n) break;
            tag=bytes[offset+6]; offset+=4+length;
        }
        if(tag==atoi(selected)) {
            if(getenv("S412_STORAGE_FAIL")) { errno=EIO; return -1; }
            gate();
        }
    }
    return original(fd);
}
int close(int fd) {
    static int (*original)(int);
    if(!original) original=dlsym(RTLD_NEXT,"close");
    if(getenv("S412_JOIN_FAIL") && journal(fd)) atomic_store(&journal_thread,(uintptr_t)pthread_self());
    if(getenv("S412_CLOSE_GATE") && journal(fd)) gate();
    return original(fd);
}
