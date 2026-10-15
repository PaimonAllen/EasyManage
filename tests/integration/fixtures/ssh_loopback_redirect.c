/* Test-only socket shim: keep SSH fixtures on loopback in a private netns.
 * Never preload this into a production process. Other connections are unchanged. */
#define _GNU_SOURCE
#include <arpa/inet.h>
#include <dlfcn.h>
#include <sys/socket.h>

int connect(int fd, const struct sockaddr *address, socklen_t length) {
    int (*original)(int, const struct sockaddr *, socklen_t) = dlsym(RTLD_NEXT, "connect");
    if (address && address->sa_family == AF_INET && length >= sizeof(struct sockaddr_in)) {
        const struct sockaddr_in *source = (const struct sockaddr_in *)address;
        unsigned long ip = ntohl(source->sin_addr.s_addr);
        if ((ip == 0x0ade0009 || ip == 0x0ade000a) && ntohs(source->sin_port) == 2222) {
            struct sockaddr_in copy = *source;
            copy.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
            return original(fd, (const struct sockaddr *)&copy, sizeof(copy));
        }
    }
    return original(fd, address, length);
}
