/* Trusted static fixture program for actual gVisor adapter tests. It is never
 * an application tool, installer or native-host fallback. */
#define _GNU_SOURCE
#include <arpa/inet.h>
#include <errno.h>
#include <fcntl.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <unistd.h>

static void put(const char *path, const void *data, size_t size) {
    int fd = open(path, O_WRONLY | O_CREAT | O_TRUNC | O_NOFOLLOW, 0600);
    if (fd < 0) { perror("create result"); exit(53); }
    const char *p = data;
    while (size) { ssize_t n = write(fd, p, size); if (n <= 0) { perror("write result"); exit(54); } p += n; size -= (size_t)n; }
    if (fsync(fd) || close(fd)) { perror("sync result"); exit(55); }
}
static int network_denied(const char *ip) {
    int fd = socket(AF_INET, SOCK_STREAM | SOCK_NONBLOCK, 0);
    if (fd < 0) return 1;
    struct sockaddr_in address = {.sin_family = AF_INET, .sin_port = htons(80)};
    if (inet_pton(AF_INET, ip, &address.sin_addr) != 1) exit(56);
    int result = connect(fd, (struct sockaddr *)&address, sizeof(address));
    int saved = errno; close(fd);
    return result < 0 && saved != EINPROGRESS;
}
static int relative_alias_file(const char *path) {
    size_t size = strlen(path);
    if (!size || size > 512 || path[0] == '/' || path[size - 1] == '/' || !strchr(path, '/')) return 0;
    const char *part = path;
    for (const char *p = path; ; ++p) {
        if (*p == '/' || !*p) {
            size_t n = (size_t)(p - part);
            if (!n || (n == 1 && part[0] == '.') || (n == 2 && part[0] == '.' && part[1] == '.')) return 0;
            if (!*p) break;
            part = p + 1;
        } else if (!( (*p >= 'a' && *p <= 'z') || (*p >= 'A' && *p <= 'Z') || (*p >= '0' && *p <= '9') || *p == '-' || *p == '_' || *p == '.')) return 0;
    }
    return 1;
}
int main(int argc, char **argv) {
    int owner_copy = argc == 5 && !strcmp(argv[1], "owner_copy");
    int owner_block = argc == 5 && !strcmp(argv[1], "owner_block");
    int owner_stop_read = argc == 5 && !strcmp(argv[1], "owner_stop_read");
    int owner = owner_copy || owner_block || owner_stop_read;
    if ((argc != 3 && !owner) || getuid() != 1000 || access(argv[owner ? 4 : 2], F_OK) == 0 || access("/mnt/c", F_OK) == 0
        || access("/var/run/docker.sock", F_OK) == 0 || access("/run/containerd/containerd.sock", F_OK) == 0
        || getenv("COWORK_TEST_HOST_SECRET") != NULL) return 51;
    if (owner) {
        if (!relative_alias_file(argv[2]) || !relative_alias_file(argv[3])) return 71;
        if (!network_denied("1.1.1.1") || !network_denied("169.254.169.254")) return 52;
        int fd = open(argv[2], O_RDONLY | O_NOFOLLOW);
        if (fd < 0) return 57;
        char data[4096]; ssize_t n = read(fd, data, sizeof(data)); close(fd);
        if (n <= 0 || n == (ssize_t)sizeof(data)) return 58;
        put(argv[3], data, (size_t)n);
        if (owner_block || owner_stop_read) {
            static const char started[] = "isolated-owner-block-started\n";
            if (owner_stop_read) {
                /* Keep all exported files inside the one confirmed alias. */
                const char *parent = strrchr(argv[3], '/');
                size_t prefix = (size_t)(parent - argv[3]) + 1;
                char marker[520];
                if (prefix + sizeof("started") > sizeof(marker)) return 72;
                memcpy(marker, argv[3], prefix);
                memcpy(marker + prefix, "started", sizeof("started"));
                put(marker, "started", 7);
            } else {
                put("/workspace/started", "started", 7);
            }
            if (write(STDOUT_FILENO, started, sizeof(started) - 1) != (ssize_t)(sizeof(started) - 1)) return 59;
            for (;;) pause();
        }
        if (write(STDOUT_FILENO, "isolated-owner-copy-complete\n", 29) != 29) return 59;
        return 0;
    }
    if (!strcmp(argv[1], "copy")) {
        if (!network_denied("1.1.1.1") || !network_denied("169.254.169.254")) return 52;
        int fd = open("/workspace/input.txt", O_RDONLY | O_NOFOLLOW);
        if (fd < 0) return 57;
        char data[4096]; ssize_t n = read(fd, data, sizeof(data)); close(fd);
        if (n <= 0 || n == (ssize_t)sizeof(data)) return 58;
        put("/workspace/result.txt", data, (size_t)n);
        put("/tmp/private-temporary", "temporary", 9);
        ssize_t written = write(STDOUT_FILENO, "isolated-program-complete\n", 26);
        if (written != 26) { char status[80]; int n=snprintf(status,sizeof(status),"stdout written=%ld errno=%d\n",(long)written,errno); put("/workspace/stdout-status",status,(size_t)n); return 59; }
        if (write(STDERR_FILENO, "separate-stderr\n", 16) != 16) return 60;
        return 0;
    }
    if (!strcmp(argv[1], "symlink_result")) {
        if (symlink("input.txt", "/workspace/unsupported-link") != 0) return 70;
        return 0;
    }
    if (!strcmp(argv[1], "block")) {
        put("/workspace/started", "started", 7);
        for (;;) pause();
    }
    if (!strcmp(argv[1], "exit128")) return 128;
    if (!strcmp(argv[1], "background")) {
        pid_t child = fork(); if (child < 0) return 61;
        if (child == 0) for (;;) pause();
        put("/workspace/child-created", "child", 5); return 0;
    }
    if (!strcmp(argv[1], "output")) {
        char data[8192]; memset(data, 'x', sizeof(data));
        for (int i = 0; i < 1024; ++i) {
            ssize_t n = write(STDOUT_FILENO, data, sizeof(data)); if (n < 0) return 62;
        }
        return 0;
    }
    if (!strcmp(argv[1], "disk")) {
        int fd = open("/workspace/full.bin", O_WRONLY | O_CREAT | O_TRUNC, 0600);
        if (fd < 0) return 63;
        char data[65536]; memset(data, 'd', sizeof(data));
        size_t total = 0;
        while (total < 128u * 1024u * 1024u) {
            ssize_t n = write(fd, data, sizeof(data));
            if (n < 0) { int saved = errno; close(fd); return saved == ENOSPC ? 0 : 64; }
            total += (size_t)n;
        }
        close(fd); return 65;
    }
    return 50;
}
