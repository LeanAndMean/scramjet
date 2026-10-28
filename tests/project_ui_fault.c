#define _GNU_SOURCE
#include <dlfcn.h>
#include <unistd.h>
#include <stdlib.h>
#include <string.h>
#include <errno.h>
#include <stdio.h>
#include <fcntl.h>
#include <sys/syscall.h>
#include <sys/socket.h>

static int project_journal(int fd) {
    char path[64], name[4096];
    snprintf(path, sizeof(path), "/proc/self/fd/%d", fd);
    ssize_t n = readlink(path, name, sizeof(name)-1);
    if (n < 0) return 0;
    name[n] = 0;
    return strstr(name, "/storage/.project-journal") != NULL;
}
static void gate(void) {
    const char *ready = getenv("S413B_READY_FD"), *release = getenv("S413B_RELEASE_FD");
    if (ready && release) { char byte = '1'; write(atoi(ready), &byte, 1); read(atoi(release), &byte, 1); }
}
ssize_t send(int fd,const void *data,size_t size,int flags) {
    static ssize_t (*actual)(int,const void *,size_t,int);
    static int fired;
    if (!actual) actual=dlsym(RTLD_NEXT,"send");
    if (!fired && getenv("S413B_READER_GATE") && size>=8 && !memcmp(data,"HTTP/1.1",8) && memmem(data,size,"\"child\":0",9)) { fired=1; gate(); }
    return actual(fd,data,size,flags);
}
int fsync(int fd) {
    static int (*actual)(int);
    static int fired;
    if (!actual) actual = dlsym(RTLD_NEXT, "fsync");
    const char *tag = getenv("S413B_STORAGE_TAG");
    if (tag && !fired && project_journal(fd)) {
        char data[262145];
        char descriptor[64];
        snprintf(descriptor,sizeof(descriptor),"/proc/self/fd/%d",fd);
        int reader = open(descriptor,O_RDONLY|O_CLOEXEC);
        ssize_t n = reader < 0 ? -1 : pread(reader, data, sizeof(data)-1, 0);
        if (reader >= 0) syscall(SYS_close,reader);
        if (n > 0) {
            data[n] = 0;
            char *last = data+n-1;
            while (last > data && last[-1] != '\n') last--;
            if (strstr(last,tag)) {
                fired = 1;
                gate();
                if (getenv("S413B_STORAGE_FAIL")) { errno = EIO; return -1; }
            }
        }
    }
    return actual(fd);
}
int close(int fd) {
    static int (*actual)(int);
    static int fired;
    if (!actual) actual = dlsym(RTLD_NEXT,"close");
    if (!fired && getenv("S413B_CLOSE_GATE") && project_journal(fd)) { fired=1; gate(); }
    return actual(fd);
}
