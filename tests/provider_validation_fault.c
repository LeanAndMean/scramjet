#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/syscall.h>
#include <sys/stat.h>
#include <unistd.h>
#include <stdatomic.h>

static atomic_int ledger_syncs;
static atomic_int partial_written;
static int owned_path(int fd, char *path) {
    char link[64];
    const char *root=getenv("VALIDATION_FAULT_ROOT");
    if (!root) return 0;
    snprintf(link,sizeof(link),"/proc/self/fd/%d",fd);
    ssize_t n=readlink(link,path,PATH_MAX-1);
    if (n<0) return 0;
    path[n]=0;
    size_t length=strlen(root);
    return strncmp(path,root,length)==0 && path[length]=='/';
}
ssize_t pwrite64(int fd,const void *bytes,size_t length,off64_t offset) {
    ssize_t (*real_pwrite)(int,const void*,size_t,off64_t)=dlsym(RTLD_NEXT,"pwrite64");
    const char *mode=getenv("VALIDATION_FAULT_MODE");char path[PATH_MAX];
    if(mode && strcmp(mode,"partial-reserve5")==0 && owned_path(fd,path) && strstr(path,"/ledger/provider-attempts.jsonl")) {
        if(atomic_load(&partial_written)){errno=EIO;return -1;}
        if(memmem(bytes,length,"\"ordinal\":5",11)) {atomic_store(&partial_written,1);return real_pwrite(fd,bytes,length/2,offset);}
    }
    return real_pwrite(fd,bytes,length,offset);
}
int fsync(int fd) {
    int (*real_fsync)(int)=dlsym(RTLD_NEXT,"fsync");
    const char *mode=getenv("VALIDATION_FAULT_MODE");
    char path[PATH_MAX];
    if (mode && owned_path(fd,path) && strstr(path,"/ledger/provider-attempts.jsonl")) {
        int count=atomic_fetch_add(&ledger_syncs,1)+1;
        int at=0;
        if (strstr(mode,"reserve5")) at=2;
        if (strstr(mode,"join5")) at=3;
        if (strstr(mode,"reserve6")) at=4;
        if (strstr(mode,"join6")) at=5;
        if (count==at) {
            if (strncmp(mode,"lose-",5)==0) {int result=real_fsync(fd);if(result==0)_exit(17);return result;}
            errno=EIO;return -1;
        }
    }
    if(mode && strcmp(mode,"hold-first")==0 && owned_path(fd,path) && strstr(path,"/evidence/first/response.data")) {
        int result=real_fsync(fd);char ready[PATH_MAX],release[PATH_MAX];const char *root=getenv("VALIDATION_FAULT_ROOT");
        int a=snprintf(ready,sizeof(ready),"%s/holder.ready",root),b=snprintf(release,sizeof(release),"%s/holder.release",root);
        if(a>0 && b>0 && (size_t)a<sizeof(ready) && (size_t)b<sizeof(release)){int marker=open(ready,O_CREAT|O_EXCL|O_WRONLY,0600);if(marker>=0)syscall(SYS_close,marker);for(int i=0;i<5000 && access(release,F_OK)!=0;i++)usleep(1000);}
        return result;
    }
    return real_fsync(fd);
}
ssize_t write(int fd,const void *bytes,size_t length) {
    ssize_t (*real_write)(int,const void*,size_t)=dlsym(RTLD_NEXT,"write");
    const char *mode=getenv("VALIDATION_FAULT_MODE");char path[PATH_MAX];
    if (mode && strcmp(mode,"raw-write")==0 && owned_path(fd,path) && strstr(path,"/evidence/first/response.data")) {errno=EIO;return -1;}
    return real_write(fd,bytes,length);
}
int close(int fd) {
    int (*real_close)(int)=dlsym(RTLD_NEXT,"close");
    const char *mode=getenv("VALIDATION_FAULT_MODE");char path[PATH_MAX];
    struct stat stat;
    if (!mode || !owned_path(fd,path) || (fcntl(fd,F_GETFL)&O_ACCMODE)==O_RDONLY || fstat(fd,&stat)!=0 || stat.st_size==0) return real_close(fd);
    int raw=strstr(path,"/evidence/first/response.data")!=NULL;
    int summary=strstr(path,"/evidence/summary.json")!=NULL;
    int request=strstr(path,"/evidence/first/request.json")!=NULL;
    if(summary && strcmp(mode,"summary-replace-root")==0) {
        char saved[PATH_MAX];char *slash=strrchr(path,'/');if(slash){*slash=0;size_t length=strlen(path);if(length+7<sizeof(saved)){memcpy(saved,path,length);memcpy(saved+length,".saved",7);if(rename(path,saved)==0)mkdir(path,0700);}}
    }
    if(summary && strcmp(mode,"summary-mutate-first")==0) {
        char raw_path[PATH_MAX];int length=snprintf(raw_path,sizeof(raw_path),"%s/evidence/first/response.data",getenv("VALIDATION_FAULT_ROOT"));
        if(length>0 && (size_t)length<sizeof(raw_path)){int raw_fd=open(raw_path,O_WRONLY|O_NOFOLLOW);if(raw_fd>=0){if(pwrite64(raw_fd,"!",1,0)!=1)_exit(99);real_close(raw_fd);}}
    }
    if ((raw && strcmp(mode,"raw-close")==0) || (summary && strcmp(mode,"summary-close")==0)) {real_close(fd);errno=EIO;return -1;}
    if (raw && strcmp(mode,"replace-directory")==0) {
        char saved[PATH_MAX];char *slash=strrchr(path,'/');
        if (slash) {*slash=0;size_t length=strlen(path);if(length+7<sizeof(saved)){memcpy(saved,path,length);memcpy(saved+length,".saved",7);if(rename(path,saved)==0)mkdir(path,0700);}}
    }
    if ((raw && strcmp(mode,"replace-first")==0) || (request && strcmp(mode,"replace-request")==0)) {
        char saved[PATH_MAX];
        size_t length=strlen(path);
        if (length+7<sizeof(saved)) {
            memcpy(saved,path,length);memcpy(saved+length,".saved",7);
            if (rename(path,saved)==0) {int replacement=open(path,O_CREAT|O_EXCL|O_WRONLY,0600);if(replacement>=0)real_close(replacement);}
        }
    }
    return real_close(fd);
}
