#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <stdatomic.h>
static _Atomic int failed_transport;
static _Atomic int injected_first;
static _Atomic int reply_number;
static _Atomic int root_syncs;

ssize_t send(int fd, const void *data, size_t size, int flags) {
    static ssize_t (*original)(int, const void *, size_t, int);
    if (!original) original=dlsym(RTLD_NEXT,"send");
    int header = size >= 12 && size <= 1024 && !memcmp(data,"HTTP/1.1 200",12);
    const char *turn = getenv("S49_FAIL_TURN");
    int selected = header && turn && atomic_fetch_add(&reply_number,1)+1 == atoi(turn);
    if (header && (selected || getenv("S43_TRANSPORT_FAIL") || (getenv("S48_FAIL_FIRST") && !atomic_exchange(&injected_first,1)))) {
        char copy[1024]; memcpy(copy,data,size); memcpy(copy+9,"500",3);
        atomic_store(&failed_transport,1);
        return original(fd,copy,size,flags);
    }
    return original(fd,data,size,flags);
}

int fsync(int fd) {
    static int (*original)(int);
    if (!original) original = dlsym(RTLD_NEXT, "fsync");
    const char *gate = getenv("S43_STORAGE_GATE");
    const char *marker_gate = getenv("S49_MARKER_GATE");
    if (marker_gate) gate = marker_gate;
    if (getenv("S49_UNCERTAIN_SECOND") || getenv("S410_UNCERTAIN_FIRST")) {
        struct stat st;
        int selected = getenv("S410_UNCERTAIN_FIRST") ? 2 : 3;
        if (!fstat(fd, &st) && S_ISDIR(st.st_mode) && atomic_fetch_add(&root_syncs,1)+1 == selected) {
            errno=EIO; return -1;
        }
    }
    if (gate) {
        char descriptor[64], target[4096], entered[4096], released[4096];
        snprintf(descriptor, sizeof(descriptor), "/proc/self/fd/%d", fd);
        ssize_t n = readlink(descriptor, target, sizeof(target)-1);
        if (n > 0) {
            target[n] = 0;
            size_t length = strlen(target);
            char tail[513];
            struct stat st;
            int follow_marker = 0;
            if (marker_gate && !fstat(fd,&st) && S_ISREG(st.st_mode)) {
                off_t start = st.st_size > 512 ? st.st_size-512 : 0;
                int reader = open(descriptor, O_RDONLY|O_CLOEXEC);
                ssize_t count = reader < 0 ? -1 : pread(reader,tail,512,start);
                if (reader >= 0) close(reader);
                if (count > 0) { tail[count]=0; follow_marker = strstr(tail,"\"kind\":\"FollowUp\"") != NULL; }
            }
            if (follow_marker || (!marker_gate && ((length >= 14 && !strcmp(target+length-14, "/.coding-stage")) || (atomic_load(&failed_transport) && length >= 16 && !strcmp(target+length-16, "/.coding-journal"))))) {
                if (snprintf(entered,sizeof(entered),"%s.entered",gate) >= (int)sizeof(entered) || snprintf(released,sizeof(released),"%s.release",gate) >= (int)sizeof(released)) { errno=EIO; return -1; }
                int marker = open(entered, O_WRONLY|O_CREAT|O_EXCL,0600);
                if (marker < 0) return -1;
                close(marker);
                int attempts = getenv("S47_WATCHDOG_GAP") ? 4000 : 500;
                for (int i=0;i<attempts;i++) {
                    if (!access(released,F_OK)) return original(fd);
                    struct timespec wait={0,10000000}; nanosleep(&wait,NULL);
                }
                errno=ETIMEDOUT; return -1;
            }
        }
    }
    return original(fd);
}
