#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <fcntl.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

ssize_t write(int fd, const void *buffer, size_t size) {
    static ssize_t (*real_write)(int, const void *, size_t);
    static int failed;
    if (!real_write) real_write = dlsym(RTLD_NEXT, "write");
    const char *kind = getenv("S13_FAULT_KIND");
    const char disable[] = "\033[?2004l\033[?1004l\033[?1006l\033[?1002l\033[?25h\033[?1049l";
    int cursor = !kind && size == 6 && memcmp(buffer, "\033[?25h", 6) == 0;
    int restore = kind && strcmp(kind, "restore") == 0 && size == sizeof(disable) - 1 && memcmp(buffer, disable, size) == 0;
    int report = kind && strcmp(kind, "report") == 0 && size >= 13 && memcmp(buffer, "UI-DIAGNOSTIC", 13) == 0;
    if (!failed && fd == STDOUT_FILENO && (cursor || restore || report)) {
        failed = 1;
        const char *path = getenv("S12_FAULT_RECEIPT");
        int receipt = path ? open(path, O_WRONLY | O_CREAT | O_TRUNC, 0600) : -1;
        if (receipt >= 0) {
            const char *message = cursor ? "exact stdout cursor-show write failed once\n" : restore ? "exact stdout mode-disable write failed once\n" : "exact stdout metadata-report write failed once\n";
            real_write(receipt, message, strlen(message));
            close(receipt);
        }
        errno = EIO;
        return -1;
    }
    return real_write(fd, buffer, size);
}
