#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

int fsync(int fd) {
    int (*real_fsync)(int) = dlsym(RTLD_NEXT, "fsync");
    const char *target = getenv("SCRAMJET_FAULT_TARGET");
    const char *kind = getenv("SCRAMJET_FAULT_KIND");
    const char *mode = getenv("SCRAMJET_FAULT_MODE");
    char link[64], path[4096];
    snprintf(link, sizeof(link), "/proc/self/fd/%d", fd);
    ssize_t length = readlink(link, path, sizeof(path) - 1);
    if (!target || !kind || !mode || length < 0) return real_fsync(fd);
    path[length] = 0;
    if (strcmp(path, target)) return real_fsync(fd);
    int match = !strcmp(kind, "file");
    if (!match) {
        struct stat stat;
        static char bytes[131073];
        if (fstat(fd, &stat) || stat.st_size <= 0) return real_fsync(fd);
        size_t count = stat.st_size > 131072 ? 131072 : (size_t)stat.st_size;
        ssize_t read = pread(fd, bytes, count, stat.st_size - count);
        if (read <= 0) return real_fsync(fd);
        bytes[read] = 0;
        if (bytes[read - 1] == '\n') bytes[read - 1] = 0;
        char *last = strrchr(bytes, '\n');
        char pattern[128];
        snprintf(pattern, sizeof(pattern), "\"kind\":\"%s\"", kind);
        match = strstr(last ? last + 1 : bytes, pattern) != NULL;
    }
    if (!match) return real_fsync(fd);
    if (!strcmp(mode, "error")) { errno = EIO; return -1; }
    int result = real_fsync(fd);
    if (!strcmp(mode, "exit")) _exit(73);
    return result;
}
