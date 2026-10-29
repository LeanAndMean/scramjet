#define _GNU_SOURCE
#include <dlfcn.h>
#include <unistd.h>
#include <stdlib.h>
#include <string.h>
#include <errno.h>
#include <stdio.h>
#include <fcntl.h>
#include <sys/syscall.h>

static int journal(int fd, int provider) {
    char path[64], name[4096];
    snprintf(path, sizeof(path), "/proc/self/fd/%d", fd);
    ssize_t n = readlink(path, name, sizeof(name)-1);
    if (n < 0) return 0;
    name[n] = 0;
    const char *suffix = provider ? "/provider-attempts.jsonl" : "/haiku.jsonl";
    return (size_t)n >= strlen(suffix) && !strcmp(name+n-strlen(suffix), suffix);
}
static int record_kind(const char *text, const char *tag, int target_depth) {
    int depth = 0, quoted = 0, escaped = 0;
    size_t length = strlen(tag);
    for (const char *p = text; *p; p++) {
        if (quoted) {
            if (escaped) escaped = 0;
            else if (*p == '\\') escaped = 1;
            else if (*p == '"') quoted = 0;
            continue;
        }
        if (*p == '"') {
            if (depth == target_depth && !strncmp(p, "\"kind\":\"", 8)) {
                return !strncmp(p+8, tag, length) && p[8+length] == '"';
            }
            quoted = 1;
        } else if (*p == '{') depth++;
        else if (*p == '}') depth--;
    }
    return 0;
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
    const char *provider_tag = getenv("S414C_PROVIDER_TAG");
    const char *tag = provider_tag ? provider_tag : getenv("S414C_STORAGE_TAG");
    if (tag && !fired && journal(fd, provider_tag != NULL)) {
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
            if (record_kind(last, tag, provider_tag ? 1 : 2)) {
                fired = 1;
                gate();
                if (getenv("S414C_STORAGE_FAIL")) { errno = EIO; return -1; }
            }
        }
    }
    return actual(fd);
}
ssize_t write(int fd, const void *data, size_t size) {
    static ssize_t (*actual)(int, const void *, size_t);
    static int enabled, failed, restored, paint_matched, paint_fired;
    if (!actual) actual = dlsym(RTLD_NEXT, "write");
    if (!paint_fired && getenv("S414C_PAINT_GATE") && fd > 2 && isatty(fd)) {
        const char *title = "EXACT APPROVAL";
        const unsigned char *bytes = data;
        for (size_t i = 0; i < size; i++) {
            if (bytes[i] == (unsigned char)title[paint_matched]) paint_matched++;
            else paint_matched = bytes[i] == 'E' ? 1 : 0;
            if (!title[paint_matched]) { paint_fired = 1; gate(); break; }
        }
    }
    const char *kind = getenv("S414C_WRITER_FAULT");
    if (kind && fd > 2 && isatty(fd)) {
        if (memmem(data, size, "?1049h", 6)) {
            if (!strcmp(kind, "init")) { errno = EIO; return -1; }
            enabled = 1;
            if (!strcmp(kind, "stall")) return actual(fd, data, size);
        }
        if (memmem(data, size, "?1049l", 6)) {
            restored = 1;
            if (!strcmp(kind, "restore")) { errno = EIO; return -1; }
        }
        if (enabled && !restored && !strcmp(kind, "stall")) { errno = EAGAIN; return -1; }
        if (!failed && restored && ((!strcmp(kind, "cursor") && size == 6 && !memcmp(data, "\033[?25h", 6)) || (!strcmp(kind, "report") && memmem(data, size, "haiku-ui: terminal_returned", 26)))) {
            failed = 1; errno = EIO; return -1;
        }
    }
    return actual(fd, data, size);
}
int close(int fd) {
    static int (*actual)(int);
    static int fired;
    if (!actual) actual = dlsym(RTLD_NEXT, "close");
    if (!fired && getenv("S414C_CLOSE_GATE") && journal(fd, 0)
        && (fcntl(fd, F_GETFL) & O_ACCMODE) == O_WRONLY) { fired = 1; gate(); }
    return actual(fd);
}
