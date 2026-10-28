#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

static int enabled;
static int restored;
static int contains(const void *data, size_t size, const char *needle) {
    return memmem(data, size, needle, strlen(needle)) != NULL;
}
ssize_t write(int fd, const void *data, size_t size) {
    static ssize_t (*original)(int, const void *, size_t);
    if (!original) original = dlsym(RTLD_NEXT, "write");
    const char *kind = getenv("S41_WRITE_FAULT");
    if (kind && isatty(fd)) {
        if (contains(data, size, "?1049h")) {
            if (!strcmp(kind, "init")) { errno = EIO; return -1; }
            enabled += 1;
            if (enabled == 2 && !strcmp(kind, "reentry_init")) { errno = EIO; return -1; }
        } else if (enabled && fd > 2 && !strcmp(kind, "stall")) { errno = EAGAIN; return -1; }
        if (contains(data, size, "?1049l")) {
            restored = 1;
            if (!strcmp(kind, "restore") || (enabled == 2 && !strcmp(kind, "reentry_restore"))) { errno = EIO; return -1; }
        }
        if (restored && fd > 2 && !strcmp(kind, "report") && (contains(data, size, "runtime-ui-demo:") || contains(data, size, "reader-ui-demo:") || contains(data, size, "coding-ui-demo:") || contains(data, size, "command-ui-demo:"))) { errno = EIO; return -1; }
    }
    return original(fd, data, size);
}
