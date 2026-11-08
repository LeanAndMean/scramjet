/* A stand-in bash.exe for scripts/try-windows.py on a Windows without Git for Windows: it runs
 * `-c COMMAND` through cmd.exe (so in cmd's syntax), which starts a real tree of processes. */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

int main(int argc, char **argv) {
    if (argc == 3 && strcmp(argv[1], "-c") == 0) {
        fflush(stdout);
        return system(argv[2]);
    }
    fprintf(stderr, "stand-in bash: only -c COMMAND\n");
    return 2;
}
