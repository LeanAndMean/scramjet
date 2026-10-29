#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

static int targeted(int fd) {
    const char *target = getenv("SCRAMJET_FAULT_TARGET");
    const char *suffix = getenv("SCRAMJET_FAULT_SUFFIX");
    char link[64], path[4096];
    snprintf(link, sizeof(link), "/proc/self/fd/%d", fd);
    ssize_t length = readlink(link, path, sizeof(path) - 1);
    if (length < 0) return 0;
    path[length] = 0;
    if (target) return !strcmp(path, target);
    return suffix && (size_t)length >= strlen(suffix) && !strcmp(path + length - strlen(suffix), suffix);
}
int fsync(int fd) {
    int (*real_fsync)(int) = dlsym(RTLD_NEXT, "fsync");
    const char *kind = getenv("SCRAMJET_FAULT_KIND");
    const char *mode = getenv("SCRAMJET_FAULT_MODE");
    if (!kind || !mode || !targeted(fd)) return real_fsync(fd);
    int match = !strcmp(kind, "file");
    if (!strcmp(kind, "marker")) {
        struct stat stat;
        match = !fstat(fd, &stat) && stat.st_size > 0;
        if (!match) return real_fsync(fd);
    }
    if (!match && strcmp(kind, "marker")) {
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
        if (!strcmp(mode, "cancel-stop-error") && strstr(last ? last + 1 : bytes, "\"kind\":\"Stopped\"")) { errno = EIO; return -1; }
    }
    if (!match) return real_fsync(fd);
    if (!strcmp(mode, "error")) { errno = EIO; return -1; }
    int result = real_fsync(fd);
    if (!strcmp(mode, "exit")) _exit(73);
    if (!strcmp(mode, "cancel") || !strcmp(mode, "cancel-stop-error")) raise(SIGTERM);
    if (!strcmp(mode, "delay")) usleep(100000);
    return result;
}
ssize_t pread64(int fd, void *bytes, size_t count, off64_t offset) {
    ssize_t (*real_read)(int, void *, size_t, off64_t) = dlsym(RTLD_NEXT, "pread64");
    ssize_t result = real_read(fd, bytes, count, offset);
    const char *mode = getenv("SCRAMJET_FAULT_MODE");
    if (mode && !strcmp(mode, "read-cancel") && targeted(fd)) raise(SIGTERM);
    return result;
}
ssize_t write(int fd, const void *bytes, size_t count) {
    ssize_t (*real_write)(int, const void *, size_t) = dlsym(RTLD_NEXT, "write");
    const char *mode = getenv("SCRAMJET_FAULT_MODE");
    static int partial;
    if (fd == STDOUT_FILENO && mode && !strcmp(mode, "stdout-partial")) {
        if (partial) { errno = EIO; return -1; }
        partial = 1;
        return real_write(fd, bytes, count > 11 ? 11 : count);
    }
    return real_write(fd, bytes, count);
}
