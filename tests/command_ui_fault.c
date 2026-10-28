#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <stdatomic.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <fcntl.h>
#include <stdio.h>
static atomic_int left_reads;
static int target(int fd, const char *suffix) {
    char path[64], name[4096];
    snprintf(path, sizeof(path), "/proc/self/fd/%d", fd);
    ssize_t n = readlink(path, name, sizeof(name) - 1);
    if (n < 0) return 0;
    name[n] = 0;
    size_t s = strlen(suffix);
    return (size_t)n >= s && !strcmp(name + n - s, suffix);
}
ssize_t read(int fd, void *data, size_t size) {
    static ssize_t (*original)(int, void *, size_t);
    if (!original) original = dlsym(RTLD_NEXT, "read");
    const char *kind = getenv("S45_READ_FAULT");
    if (kind && size == 1025 && target(fd, "/inputs/left.txt")) {
        int call = atomic_fetch_add(&left_reads, 1);
        if ((!strcmp(kind, "gate") && call == 0) || (!strcmp(kind, "gate_companion") && call == 1)) {
            char byte = 'R';
            write(atoi(getenv("S45_READY_FD")), &byte, 1);
            if (original(atoi(getenv("S45_RELEASE_FD")), &byte, 1) != 1) { errno = EIO; return -1; }
        }
        if (!strcmp(kind, "first") || (!strcmp(kind, "companion") && call == 1)) { errno = EIO; return -1; }
    }
    if (kind && !strcmp(kind, "second") && size == 1025 && target(fd, "/inputs/right.txt")) { errno = EIO; return -1; }
    if (kind && !strcmp(kind, "setup") && target(fd, "/definitions/commands.json")) { errno = EIO; return -1; }
    return original(fd, data, size);
}
int unlinkat(int fd, const char *path, int flags) {
    static int (*original)(int, const char *, int);
    if (!original) original = dlsym(RTLD_NEXT, "unlinkat");
    if (getenv("S45_CLEANUP_GATE") && flags == AT_REMOVEDIR) {
        static atomic_int gated;
        if (!atomic_exchange(&gated, 1)) {
            char byte = 'R';
            write(atoi(getenv("S45_READY_FD")), &byte, 1);
            if (read(atoi(getenv("S45_RELEASE_FD")), &byte, 1) != 1) { errno = EIO; return -1; }
        }
    }
    if (getenv("S45_CLEANUP_FAULT") && flags == AT_REMOVEDIR) { errno = EIO; return -1; }
    return original(fd, path, flags);
}
