/* Fixed, sealed per-job sandbox root. Never a host program or user-selected
 * workload. It keeps the sentry available to retain the child task's status. */
#include <unistd.h>
int main(int argc, char **argv) {
    (void)argv;
    if (argc != 1 || getuid() != 1000) return 127;
    for (;;) pause();
}
