/* A stand-in bash.exe for scripts/try-windows.py on a Windows without Git for Windows: it runs
 * the command scramjet-rs gives bash in SCRAMJET_RS_COMMAND through cmd.exe (so in cmd's
 * syntax), which starts a real tree of processes. */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

int main(int argc, char **argv) {
    const char *command = getenv("SCRAMJET_RS_COMMAND");
    if (argc == 3 && strcmp(argv[1], "-c") == 0 && command) {
        fflush(stdout);
        return system(command);
    }
    fprintf(stderr, "stand-in bash: only -c with SCRAMJET_RS_COMMAND\n");
    return 2;
}
