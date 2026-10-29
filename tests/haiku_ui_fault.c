#define _GNU_SOURCE
#include <dlfcn.h>
#include <unistd.h>
#include <stdlib.h>
#include <string.h>
#include <errno.h>
#include <stdio.h>
#include <fcntl.h>
#include <sys/syscall.h>

static int journal(int fd) {
    char path[64], name[4096];
    snprintf(path, sizeof(path), "/proc/self/fd/%d", fd);
    ssize_t n = readlink(path, name, sizeof(name)-1);
    if (n < 0) return 0;
    name[n] = 0;
    const char *suffix = "/haiku.jsonl";
    return (size_t)n >= strlen(suffix) && !strcmp(name+n-strlen(suffix), suffix);
}
static void gate(void) {
    const char *ready = getenv("S414C_READY_FD"), *release = getenv("S414C_RELEASE_FD");
    if (ready && release) {
        char byte = '1';
        syscall(SYS_write, atoi(ready), &byte, 1);
        syscall(SYS_read, atoi(release), &byte, 1);
    }
}
int fsync(int fd) {
    static int (*actual)(int);
    static int fired;
    if (!actual) actual = dlsym(RTLD_NEXT, "fsync");
    const char *tag = getenv("S414C_STORAGE_TAG");
    if (tag && !fired && journal(fd)) {
        char data[131073], descriptor[64];
        snprintf(descriptor, sizeof(descriptor), "/proc/self/fd/%d", fd);
        int reader = open(descriptor, O_RDONLY|O_CLOEXEC);
        off_t end = reader < 0 ? -1 : lseek(reader, 0, SEEK_END);
        off_t start = end > 131072 ? end - 131072 : 0;
        ssize_t n = reader < 0 ? -1 : pread(reader, data, sizeof(data)-1, start);
        if (reader >= 0) syscall(SYS_close, reader);
        if (n > 0) {
            data[n] = 0;
            char *last = data+n-1;
            while (last > data && last[-1] != '\n') last--;
            if (strstr(last, tag)) {
                fired = 1;
                gate();
                if (getenv("S414C_STORAGE_FAIL")) { errno = EIO; return -1; }
            }
        }
    }
    return actual(fd);
}
int close(int fd) {
    static int (*actual)(int);
    static int fired;
    if (!actual) actual = dlsym(RTLD_NEXT, "close");
    if (!fired && getenv("S414C_CLOSE_GATE") && journal(fd)
        && (fcntl(fd, F_GETFL) & O_ACCMODE) == O_WRONLY) { fired = 1; gate(); }
    return actual(fd);
}
