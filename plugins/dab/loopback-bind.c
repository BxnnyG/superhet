/*
 * LD_PRELOAD shim for welle-cli: turn "listen on every address" into "listen on 127.0.0.1".
 *
 * welle-cli's web interface binds INADDR_ANY (Socket.cpp in welle.io 2.4) and has no option to
 * restrict that. The DAB plugin only talks to it over loopback, and the interface has no
 * authentication, so it must never be reachable from the LAN (host networking!). The adapter
 * starts welle-cli with this library preloaded: every IPv4 bind() to 0.0.0.0 becomes a bind to
 * 127.0.0.1; everything else passes through unchanged.
 *
 * Build: gcc -shared -fPIC -O2 -o libsuperhet-loopback-bind.so loopback-bind.c
 * Licence: AGPL-3.0-only, like the rest of Superhet (this is our code, not welle.io's).
 */
#define _GNU_SOURCE
#include <arpa/inet.h>
#include <netinet/in.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/syscall.h>
#include <unistd.h>

int bind(int fd, const struct sockaddr *addr, socklen_t len)
{
    if (addr != NULL && addr->sa_family == AF_INET && len >= sizeof(struct sockaddr_in)) {
        struct sockaddr_in copy;
        memcpy(&copy, addr, sizeof(copy));
        if (copy.sin_addr.s_addr == htonl(INADDR_ANY)) {
            copy.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
            return (int)syscall(SYS_bind, fd, (const struct sockaddr *)&copy, len);
        }
    }
    return (int)syscall(SYS_bind, fd, addr, len);
}
