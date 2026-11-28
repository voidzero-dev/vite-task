/* Integration test fixture exercising freestanding SIGSYS virtualization,
 * /proc/self/exe virtualization, PR_SET_NAME, RLIMIT_STACK, SyscallInterceptor,
 * recursive re-exec without seccomp filter stacking, and target-installed
 * SECCOMP_RET_TRAP forwarding under fspy_elf_loader.
 *
 * Subcommands (passed in argv[1]):
 *   sigaction            - overwrite SIGSYS (SIG_IGN, custom, SA_SIGINFO,
 *                          SA_RESETHAND, SIG_DFL), read oldact, verify sa_mask
 *                          stripping and round-trip on SIGUSR1, and forward
 *                          non-seccomp SIGSYS.
 *   sigsys-default-death - set SIGSYS to SIG_DFL and raise(SIGSYS); must die
 *                          with signal SIGSYS.
 *   sigmask-single       - sigprocmask (BLOCK/UNBLOCK/SETMASK/query),
 *                          sigsuspend, ppoll, pselect, epoll_pwait, and
 *                          epoll_pwait2 with SIGSYS in blocked masks.
 *   sigmask-threads      - multi-threaded pthread_sigmask isolation, mask
 *                          inheritance across pthread_create, concurrent
 *                          masked waits, and /proc/<pid>/task/<tid>/exe.
 *   proc-exe             - /proc/self/exe, /proc/thread-self/exe,
 *                          /proc/<pid>/exe, /proc/<pid>/task/<tid>/exe,
 *                          /proc/self/task/<tid>/exe, normalized paths,
 *                          dirfd/cwd relative lookups, readlink/readlinkat
 *                          edge cases, open/openat/openat2, stat/lstat/
 *                          fstatat/statx, access/faccessat/faccessat2, and
 *                          non-matching path pass-through.
 *   comm                 - verifies /proc/self/comm matches EXPECTED_COMM.
 *   deep-stack           - consumes >10 MiB of call stack to test RLIMIT_STACK.
 *   interceptor          - exercises SyscallInterceptor replay, -errno
 *                          short-circuit, and /proc/self/exe rewrite visibility
 *                          across faccessat, execve, and execveat.
 *   recursive-exec       - re-execs the launcher into self and asserts
 *                          /proc/self/status Seccomp_filters count stays
 *                          constant across recursive execs.
 *   nested-seccomp       - installs target's own SIGSYS handler and
 *                          SECCOMP_RET_TRAP filter (data=0) trapping getuid,
 *                          verifying forwarding while loader traps keep working.
 */

#define _GNU_SOURCE

#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <poll.h>
#include <pthread.h>
#include <sys/auxv.h>
#include <sched.h>
#include <signal.h>
#include <spawn.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/epoll.h>
#include <sys/mman.h>
#include <sys/prctl.h>
#include <sys/resource.h>
#include <sys/select.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <ucontext.h>
#include <unistd.h>

extern char **environ;

#ifndef O_PATH
#define O_PATH 010000000
#endif

#ifndef RESOLVE_NO_XDEV
#define RESOLVE_NO_XDEV 0x01
#define RESOLVE_NO_MAGICLINKS 0x02
#define RESOLVE_NO_SYMLINKS 0x04
#define RESOLVE_BENEATH 0x08
#define RESOLVE_IN_ROOT 0x10
#endif

#ifndef AT_SYMLINK_NOFOLLOW
#define AT_SYMLINK_NOFOLLOW 0x100
#endif

#ifndef PR_SET_NO_NEW_PRIVS
#define PR_SET_NO_NEW_PRIVS 38
#endif

#ifndef SYS_SECCOMP
#define SYS_SECCOMP 1
#endif

struct raw_sock_filter {
    uint16_t code;
    uint8_t jt;
    uint8_t jf;
    uint32_t k;
};

struct raw_sock_fprog {
    uint16_t len;
    struct raw_sock_filter *filter;
};

struct raw_open_how {
    uint64_t flags;
    uint64_t mode;
    uint64_t resolve;
};

struct raw_statx_timestamp {
    int64_t tv_sec;
    uint32_t tv_nsec;
    int32_t __reserved;
};

struct raw_statx {
    uint32_t stx_mask;
    uint32_t stx_blksize;
    uint64_t stx_attributes;
    uint32_t stx_nlink;
    uint32_t stx_uid;
    uint32_t stx_gid;
    uint16_t stx_mode;
    uint16_t __spare0[1];
    uint64_t stx_ino;
    uint64_t stx_size;
    uint64_t stx_blocks;
    uint64_t stx_attributes_mask;
    struct raw_statx_timestamp stx_atime;
    struct raw_statx_timestamp stx_btime;
    struct raw_statx_timestamp stx_ctime;
    struct raw_statx_timestamp stx_mtime;
    uint32_t stx_rdev_major;
    uint32_t stx_rdev_minor;
    uint32_t stx_dev_major;
    uint32_t stx_dev_minor;
    uint64_t stx_mnt_id;
    uint32_t stx_dio_mem_align;
    uint32_t stx_dio_offset_align;
    uint64_t __spare3[12];
};

static void write_all(int fd, const char *buf, size_t len) {
    while (len > 0) {
        ssize_t n = write(fd, buf, len);
        if (n <= 0) {
            return;
        }
        buf += n;
        len -= (size_t)n;
    }
}

static void fail(const char *msg) {
    char buf[256];
    int n = snprintf(buf, sizeof buf, "FAIL: %s (errno=%d)\n", msg, errno);
    if (n > 0) {
        write_all(STDERR_FILENO, buf, (size_t)n);
    }
    _exit(2);
}

static pid_t current_tid(void) {
    return (pid_t)syscall(SYS_gettid);
}

static const char *expected_exe_path(void) {
    const char *p = getenv("EXPECTED_EXE");
    if (p == NULL || p[0] != '/') {
        fail("missing EXPECTED_EXE");
    }
    return p;
}

static void assert_readlink_matches_expected(const char *path, const char *expected) {
    char buf[PATH_MAX];
    ssize_t n = readlink(path, buf, sizeof buf - 1);
    if (n < 0) {
        fail(path);
    }
    buf[n] = '\0';
    if (strcmp(buf, expected) != 0) {
        char msg[256];
        snprintf(msg, sizeof msg, "readlink mismatch for %s: got %s", path, buf);
        fail(msg);
    }
}

/* ---- sigaction subcommand ---- */

static volatile sig_atomic_t g_sigsys_plain_count = 0;
static volatile sig_atomic_t g_sigsys_info_count = 0;
static volatile sig_atomic_t g_sigsys_info_code = 0;
static volatile sig_atomic_t g_sigusr1_count = 0;
static volatile sig_atomic_t g_handler_readlink_ok = 0;
static const char *g_expected_exe = NULL;

static void check_readlink_in_handler(void) {
    char buf[PATH_MAX];
    ssize_t n = readlink("/proc/self/exe", buf, sizeof buf - 1);
    if (n > 0 && (size_t)n == strlen(g_expected_exe) && memcmp(buf, g_expected_exe, (size_t)n) == 0) {
        g_handler_readlink_ok = 1;
    }
}

static void sigsys_plain_handler(int sig) {
    if (sig == SIGSYS) {
        g_sigsys_plain_count++;
        check_readlink_in_handler();
    }
}

static void sigsys_info_handler(int sig, siginfo_t *info, void *ucontext) {
    (void)ucontext;
    if (sig == SIGSYS) {
        g_sigsys_info_count++;
        g_sigsys_info_code = info != NULL ? info->si_code : -1;
        check_readlink_in_handler();
    }
}

static volatile sig_atomic_t g_nodefer_depth = 0;
static volatile sig_atomic_t g_nodefer_max_depth = 0;
static volatile sig_atomic_t g_nodefer_total = 0;

static void sigsys_reentrant_handler(int sig) {
    if (sig != SIGSYS) {
        return;
    }
    g_nodefer_total++;
    g_nodefer_depth++;
    if (g_nodefer_depth > g_nodefer_max_depth) {
        g_nodefer_max_depth = g_nodefer_depth;
    }
    if (g_nodefer_total == 1) {
        raise(SIGSYS);
    }
    g_nodefer_depth--;
}

static void sigusr1_handler(int sig) {
    if (sig == SIGUSR1) {
        g_sigusr1_count++;
        /* Execute an intercepted syscall while SIGUSR1's sa_mask requested
         * SIGSYS blocked (sigfillset). Must not die! */
        check_readlink_in_handler();
    }
}

static int run_sigaction_test(void) {
    struct sigaction act;
    struct sigaction old;
    g_expected_exe = expected_exe_path();

    /* 1. Overwrite SIGSYS with SIG_IGN; oldact must be SIG_DFL. */
    memset(&act, 0, sizeof act);
    act.sa_handler = SIG_IGN;
    if (sigaction(SIGSYS, &act, &old) != 0) {
        fail("sigaction SIG_IGN");
    }
    if (old.sa_handler != SIG_DFL) {
        fail("initial oldact was not SIG_DFL");
    }
    /* Non-seccomp SIGSYS is now ignored, and intercepted syscalls still work. */
    if (raise(SIGSYS) != 0) {
        fail("raise SIGSYS under SIG_IGN");
    }
    assert_readlink_matches_expected("/proc/self/exe", g_expected_exe);

    /* 2. Install plain custom SIGSYS handler; oldact must be SIG_IGN. */
    memset(&act, 0, sizeof act);
    act.sa_handler = sigsys_plain_handler;
    sigemptyset(&act.sa_mask);
    sigaddset(&act.sa_mask, SIGUSR2);
    if (sigaction(SIGSYS, &act, &old) != 0) {
        fail("sigaction plain custom");
    }
    if (old.sa_handler != SIG_IGN) {
        fail("oldact after SIG_IGN mismatch");
    }
    g_handler_readlink_ok = 0;
    if (raise(SIGSYS) != 0) {
        fail("raise SIGSYS plain");
    }
    if (g_sigsys_plain_count != 1 || !g_handler_readlink_ok) {
        fail("plain SIGSYS handler did not run cleanly");
    }
    assert_readlink_matches_expected("/proc/self/exe", g_expected_exe);

    /* 3. Install SA_SIGINFO | SA_RESETHAND handler; oldact must be plain handler. */
    memset(&act, 0, sizeof act);
    act.sa_sigaction = sigsys_info_handler;
    act.sa_flags = SA_SIGINFO | SA_RESETHAND;
    sigemptyset(&act.sa_mask);
    if (sigaction(SIGSYS, &act, &old) != 0) {
        fail("sigaction SA_SIGINFO");
    }
    if (old.sa_handler != sigsys_plain_handler) {
        fail("oldact after plain handler mismatch");
    }
    if (!sigismember(&old.sa_mask, SIGUSR2)) {
        fail("oldact sa_mask did not preserve SIGUSR2");
    }
    g_handler_readlink_ok = 0;
    if (raise(SIGSYS) != 0) {
        fail("raise SIGSYS info");
    }
    if (g_sigsys_info_count != 1 || g_sigsys_info_code == SYS_SECCOMP || !g_handler_readlink_ok) {
        fail("SA_SIGINFO SIGSYS handler did not run cleanly");
    }

    /* Because SA_RESETHAND was set, querying SIGSYS must now report SIG_DFL,
     * while intercepted syscalls still keep working. */
    memset(&old, 0, sizeof old);
    if (sigaction(SIGSYS, NULL, &old) != 0) {
        fail("sigaction query after SA_RESETHAND");
    }
    if (old.sa_handler != SIG_DFL) {
        fail("SA_RESETHAND did not reset virtualized SIGSYS to SIG_DFL");
    }
    assert_readlink_matches_expected("/proc/self/exe", g_expected_exe);

    /* 4. Explicitly set SIG_DFL and verify intercepted syscalls still work. */
    memset(&act, 0, sizeof act);
    act.sa_handler = SIG_DFL;
    if (sigaction(SIGSYS, &act, &old) != 0) {
        fail("sigaction SIG_DFL");
    }
    if (old.sa_handler != SIG_DFL) {
        fail("oldact before explicit SIG_DFL mismatch");
    }
    assert_readlink_matches_expected("/proc/self/exe", g_expected_exe);

    /* 5. Install SIGUSR1 with sigfillset(&sa_mask) (includes SIGSYS).
     * Inside SIGUSR1 handler, execute intercepted syscall; also verify oldact
     * round-trips SIGSYS in sa_mask. */
    memset(&act, 0, sizeof act);
    act.sa_handler = sigusr1_handler;
    sigfillset(&act.sa_mask);
    if (sigaction(SIGUSR1, &act, NULL) != 0) {
        fail("sigaction SIGUSR1");
    }
    g_handler_readlink_ok = 0;
    if (raise(SIGUSR1) != 0) {
        fail("raise SIGUSR1");
    }
    if (g_sigusr1_count != 1 || !g_handler_readlink_ok) {
        fail("SIGUSR1 handler failed with SIGSYS in sa_mask");
    }
    memset(&old, 0, sizeof old);
    if (sigaction(SIGUSR1, NULL, &old) != 0) {
        fail("sigaction query SIGUSR1");
    }
    if (!sigismember(&old.sa_mask, SIGSYS)) {
        fail("SIGUSR1 oldact.sa_mask lost SIGSYS");
    }

    /* 6. Clearing SIGSYS from SIGUSR1's sa_mask must clear SA_MASK_HAS_SIGSYS. */
    memset(&act, 0, sizeof act);
    act.sa_handler = sigusr1_handler;
    sigemptyset(&act.sa_mask);
    if (sigaction(SIGUSR1, &act, NULL) != 0) {
        fail("sigaction SIGUSR1 clear sa_mask");
    }
    memset(&old, 0, sizeof old);
    if (sigaction(SIGUSR1, NULL, &old) != 0) {
        fail("sigaction query SIGUSR1 after clear");
    }
    if (sigismember(&old.sa_mask, SIGSYS) != 0) {
        fail("SIGUSR1 oldact.sa_mask retained stale SIGSYS bit");
    }

    /* 7. SA_NODEFER allows recursive SIGSYS delivery; without SA_NODEFER,
     * a nested raise(SIGSYS) is deferred until the outer handler returns. */
    memset(&act, 0, sizeof act);
    act.sa_handler = sigsys_reentrant_handler;
    act.sa_flags = SA_NODEFER;
    sigemptyset(&act.sa_mask);
    if (sigaction(SIGSYS, &act, NULL) != 0) {
        fail("sigaction SIGSYS SA_NODEFER");
    }
    g_nodefer_depth = 0;
    g_nodefer_max_depth = 0;
    g_nodefer_total = 0;
    if (raise(SIGSYS) != 0) {
        fail("raise SIGSYS SA_NODEFER");
    }
    if (g_nodefer_total != 2 || g_nodefer_max_depth != 2) {
        fail("SA_NODEFER did not allow nested SIGSYS delivery");
    }

    memset(&act, 0, sizeof act);
    act.sa_handler = sigsys_reentrant_handler;
    act.sa_flags = 0;
    sigemptyset(&act.sa_mask);
    if (sigaction(SIGSYS, &act, NULL) != 0) {
        fail("sigaction SIGSYS without SA_NODEFER");
    }
    g_nodefer_depth = 0;
    g_nodefer_max_depth = 0;
    g_nodefer_total = 0;
    if (raise(SIGSYS) != 0) {
        fail("raise SIGSYS without SA_NODEFER");
    }
    if (g_nodefer_total != 2 || g_nodefer_max_depth != 1) {
        fail("SIGSYS without SA_NODEFER was not deferred until handler return");
    }

    write_all(STDOUT_FILENO, "sigaction-ok\n", 13);
    return 0;
}

static int run_sigsys_default_death(void) {
    /* Suppress core dumps so the expected SIGSYS death finishes immediately. */
    struct rlimit zero = {0, 0};
    (void)setrlimit(RLIMIT_CORE, &zero);

    struct sigaction act;
    memset(&act, 0, sizeof act);
    act.sa_handler = SIG_DFL;
    if (sigaction(SIGSYS, &act, NULL) != 0) {
        fail("sigaction SIG_DFL");
    }
    raise(SIGSYS);
    fail("raise(SIGSYS) with SIG_DFL returned");
    return 1;
}

/* ---- sigmask-single subcommand ---- */

struct signal_sender_arg {
    pthread_t target;
    int ready_fd;
};

static void *send_sigusr1_thread(void *raw) {
    struct signal_sender_arg *arg = (struct signal_sender_arg *)raw;
    char ch = 0;
    if (read(arg->ready_fd, &ch, 1) != 1) {
        fail("sender read");
    }
    for (int i = 0; i < 200; i++) {
        if (g_sigusr1_count > 0) {
            break;
        }
        pthread_kill(arg->target, SIGUSR1);
        struct timespec ts = {0, 2 * 1000 * 1000};
        nanosleep(&ts, NULL);
    }
    return NULL;
}

static int run_sigmask_single_test(void) {
    sigset_t set, old, cur, full;
    g_expected_exe = expected_exe_path();

    /* 1. sigprocmask BLOCK / query / UNBLOCK / SETMASK */
    sigemptyset(&set);
    sigaddset(&set, SIGSYS);
    if (sigprocmask(SIG_BLOCK, &set, &old) != 0) {
        fail("sigprocmask SIG_BLOCK");
    }
    if (sigismember(&old, SIGSYS) != 0) {
        fail("SIGSYS initially blocked");
    }
    assert_readlink_matches_expected("/proc/self/exe", g_expected_exe);

    sigemptyset(&cur);
    if (sigprocmask(SIG_SETMASK, NULL, &cur) != 0) {
        fail("sigprocmask query after BLOCK");
    }
    if (sigismember(&cur, SIGSYS) != 1) {
        fail("SIGSYS not reported blocked after SIG_BLOCK");
    }

    if (sigprocmask(SIG_UNBLOCK, &set, &old) != 0) {
        fail("sigprocmask SIG_UNBLOCK");
    }
    if (sigismember(&old, SIGSYS) != 1) {
        fail("oldset did not report SIGSYS before UNBLOCK");
    }
    if (sigprocmask(SIG_SETMASK, NULL, &cur) != 0) {
        fail("sigprocmask query after UNBLOCK");
    }
    if (sigismember(&cur, SIGSYS) != 0) {
        fail("SIGSYS still reported blocked after UNBLOCK");
    }

    sigfillset(&full);
    if (sigprocmask(SIG_SETMASK, &full, &old) != 0) {
        fail("sigprocmask SIG_SETMASK full");
    }
    assert_readlink_matches_expected("/proc/self/exe", g_expected_exe);
    if (sigprocmask(SIG_SETMASK, NULL, &cur) != 0) {
        fail("sigprocmask query after SETMASK full");
    }
    if (sigismember(&cur, SIGSYS) != 1) {
        fail("SIGSYS not reported blocked after SETMASK full");
    }
    sigemptyset(&set);
    if (sigprocmask(SIG_SETMASK, &set, &old) != 0) {
        fail("sigprocmask clear");
    }
    if (sigismember(&old, SIGSYS) != 1) {
        fail("oldset lost SIGSYS before clear");
    }

    /* 1b. Non-SIGSYS signals blocked via sigprocmask must persist after
     * rt_sigreturn and actually defer delivery until unblocked; also verify
     * set == oldset aliasing. */
    {
        struct sigaction sa_u1;
        memset(&sa_u1, 0, sizeof sa_u1);
        sa_u1.sa_handler = sigusr1_handler;
        sigemptyset(&sa_u1.sa_mask);
        if (sigaction(SIGUSR1, &sa_u1, NULL) != 0) {
            fail("sigaction SIGUSR1 for persist test");
        }
        g_sigusr1_count = 0;

        sigemptyset(&set);
        sigaddset(&set, SIGUSR1);
        if (sigprocmask(SIG_BLOCK, &set, NULL) != 0) {
            fail("sigprocmask block SIGUSR1");
        }
        sigemptyset(&cur);
        if (sigprocmask(SIG_SETMASK, NULL, &cur) != 0 || sigismember(&cur, SIGUSR1) != 1) {
            fail("SIGUSR1 block did not persist after rt_sigreturn");
        }
        if (raise(SIGUSR1) != 0) {
            fail("raise blocked SIGUSR1");
        }
        if (g_sigusr1_count != 0) {
            fail("SIGUSR1 was delivered while blocked");
        }

        /* Aliasing: set == oldset when adding SIGUSR2 must return old mask
         * (SIGUSR1 set, SIGUSR2 clear) while leaving both blocked. */
        sigset_t alias;
        sigemptyset(&alias);
        sigaddset(&alias, SIGUSR2);
        if (sigprocmask(SIG_BLOCK, &alias, &alias) != 0) {
            fail("sigprocmask aliasing block");
        }
        if (sigismember(&alias, SIGUSR1) != 1 || sigismember(&alias, SIGUSR2) != 0) {
            fail("sigprocmask aliasing returned wrong oldset");
        }
        sigemptyset(&cur);
        if (sigprocmask(SIG_SETMASK, NULL, &cur) != 0
            || sigismember(&cur, SIGUSR1) != 1
            || sigismember(&cur, SIGUSR2) != 1) {
            fail("sigprocmask aliasing did not apply new mask");
        }

        sigemptyset(&set);
        sigaddset(&set, SIGUSR1);
        sigaddset(&set, SIGUSR2);
        if (sigprocmask(SIG_UNBLOCK, &set, NULL) != 0) {
            fail("sigprocmask unblock SIGUSR1+SIGUSR2");
        }
        if (g_sigusr1_count != 1) {
            fail("pending SIGUSR1 was not delivered upon unblock");
        }
    }

    /* 2. sigsuspend with SIGSYS in wait mask, interrupted by SIGUSR1 whose
     * handler runs an intercepted syscall. */
    struct sigaction sa;
    memset(&sa, 0, sizeof sa);
    sa.sa_handler = sigusr1_handler;
    sigfillset(&sa.sa_mask);
    if (sigaction(SIGUSR1, &sa, NULL) != 0) {
        fail("sigaction SIGUSR1 for sigsuspend");
    }
    sigemptyset(&set);
    sigaddset(&set, SIGUSR1);
    if (sigprocmask(SIG_BLOCK, &set, NULL) != 0) {
        fail("block SIGUSR1");
    }

    int ready_pipe[2];
    if (pipe(ready_pipe) != 0) {
        fail("pipe for sigsuspend");
    }
    g_sigusr1_count = 0;
    g_handler_readlink_ok = 0;
    struct signal_sender_arg sarg = {pthread_self(), ready_pipe[0]};
    pthread_t sender;
    if (pthread_create(&sender, NULL, send_sigusr1_thread, &sarg) != 0) {
        fail("pthread_create sender");
    }
    char go = 'g';
    if (write(ready_pipe[1], &go, 1) != 1) {
        fail("write go");
    }
    sigset_t suspend_mask;
    sigfillset(&suspend_mask);
    sigdelset(&suspend_mask, SIGUSR1);
    errno = 0;
    if (sigsuspend(&suspend_mask) != -1 || errno != EINTR) {
        fail("sigsuspend did not return EINTR");
    }
    pthread_join(sender, NULL);
    close(ready_pipe[0]);
    close(ready_pipe[1]);
    if (g_sigusr1_count == 0 || !g_handler_readlink_ok) {
        fail("SIGUSR1 handler failed during sigsuspend");
    }
    sigemptyset(&set);
    if (sigprocmask(SIG_SETMASK, &set, NULL) != 0) {
        fail("unblock after sigsuspend");
    }

    /* 3. ppoll with sigfillset (contains SIGSYS) on ready pipe + timeout */
    int pfd[2];
    if (pipe(pfd) != 0) {
        fail("pipe for ppoll");
    }
    if (write(pfd[1], "x", 1) != 1) {
        fail("write pipe");
    }
    struct pollfd poll_fd = {.fd = pfd[0], .events = POLLIN, .revents = 0};
    struct timespec tmo = {0, 10 * 1000 * 1000};
    sigfillset(&full);
    if (ppoll(&poll_fd, 1, &tmo, &full) != 1 || (poll_fd.revents & POLLIN) == 0) {
        fail("ppoll ready with full sigmask");
    }
    assert_readlink_matches_expected("/proc/self/exe", g_expected_exe);

    /* 4. pselect (pselect6) with sigfillset (contains SIGSYS) */
    fd_set rfds;
    FD_ZERO(&rfds);
    FD_SET(pfd[0], &rfds);
    if (pselect(pfd[0] + 1, &rfds, NULL, NULL, &tmo, &full) != 1 || !FD_ISSET(pfd[0], &rfds)) {
        fail("pselect with full sigmask");
    }
    assert_readlink_matches_expected("/proc/self/exe", g_expected_exe);

    /* 5. epoll_pwait (and epoll_pwait2 if supported) with sigfillset */
    int epfd = epoll_create1(EPOLL_CLOEXEC);
    if (epfd < 0) {
        fail("epoll_create1");
    }
    struct epoll_event ev;
    ev.events = EPOLLIN;
    ev.data.fd = pfd[0];
    if (epoll_ctl(epfd, EPOLL_CTL_ADD, pfd[0], &ev) != 0) {
        fail("epoll_ctl");
    }
    struct epoll_event out_ev;
    if (epoll_pwait(epfd, &out_ev, 1, 100, &full) != 1 || out_ev.data.fd != pfd[0]) {
        fail("epoll_pwait with full sigmask");
    }
#ifdef SYS_epoll_pwait2
    {
        uint64_t raw_full = ~UINT64_C(0);
        long rc = syscall(SYS_epoll_pwait2, epfd, &out_ev, 1, &tmo, &raw_full, sizeof raw_full);
        if (rc == -1 && errno == ENOSYS) {
            /* Older kernel without epoll_pwait2; skip cleanly. */
        } else if (rc != 1 || out_ev.data.fd != pfd[0]) {
            fail("epoll_pwait2 with full sigmask");
        }
    }
#endif
    assert_readlink_matches_expected("/proc/self/exe", g_expected_exe);

    close(epfd);
    close(pfd[0]);
    close(pfd[1]);
    write_all(STDOUT_FILENO, "sigmask-single-ok\n", 18);
    return 0;
}

/* ---- sigmask-threads subcommand ---- */

struct worker_arg {
    int id;
    int block_sigsys;
    pthread_barrier_t *barrier;
};

static void *child_inherits_mask_fn(void *unused) {
    (void)unused;
    sigset_t cur;
    sigemptyset(&cur);
    if (pthread_sigmask(SIG_SETMASK, NULL, &cur) != 0) {
        fail("child query mask");
    }
    if (sigismember(&cur, SIGSYS) != 1) {
        fail("child thread did not inherit blocked SIGSYS");
    }
    assert_readlink_matches_expected("/proc/self/exe", g_expected_exe);

    sigset_t only;
    sigemptyset(&only);
    sigaddset(&only, SIGSYS);
    if (pthread_sigmask(SIG_UNBLOCK, &only, NULL) != 0) {
        fail("child unblock SIGSYS");
    }
    if (pthread_sigmask(SIG_SETMASK, NULL, &cur) != 0 || sigismember(&cur, SIGSYS) != 0) {
        fail("child SIGSYS still blocked after unblock");
    }
    return NULL;
}

static void *worker_thread_fn(void *raw) {
    struct worker_arg *arg = (struct worker_arg *)raw;
    sigset_t set, cur, full;
    char path[128];

    if (arg->block_sigsys) {
        sigemptyset(&set);
        sigaddset(&set, SIGSYS);
        if (pthread_sigmask(SIG_BLOCK, &set, NULL) != 0) {
            fail("worker pthread_sigmask block");
        }
    }

    pthread_barrier_wait(arg->barrier);

    sigemptyset(&cur);
    if (pthread_sigmask(SIG_SETMASK, NULL, &cur) != 0) {
        fail("worker query mask");
    }
    if (sigismember(&cur, SIGSYS) != arg->block_sigsys) {
        fail("worker SIGSYS virtualized mask isolation broken");
    }

    assert_readlink_matches_expected("/proc/self/exe", g_expected_exe);
    assert_readlink_matches_expected("/proc/thread-self/exe", g_expected_exe);
    snprintf(path, sizeof path, "/proc/%d/task/%d/exe", (int)getpid(), (int)current_tid());
    assert_readlink_matches_expected(path, g_expected_exe);

    sigfillset(&full);
    struct timespec zero = {0, 0};
    if (ppoll(NULL, 0, &zero, &full) != 0) {
        fail("worker ppoll");
    }

    if (arg->block_sigsys && arg->id == 0) {
        pthread_t child;
        if (pthread_create(&child, NULL, child_inherits_mask_fn, NULL) != 0) {
            fail("grandchild pthread_create");
        }
        pthread_join(child, NULL);
        if (pthread_sigmask(SIG_SETMASK, NULL, &cur) != 0 || sigismember(&cur, SIGSYS) != 1) {
            fail("parent lost blocked SIGSYS after child unblocked");
        }
    }

    pthread_barrier_wait(arg->barrier);

    if (arg->block_sigsys) {
        sigemptyset(&set);
        sigaddset(&set, SIGSYS);
        if (pthread_sigmask(SIG_UNBLOCK, &set, NULL) != 0) {
            fail("worker unblock");
        }
    }
    return NULL;
}

static int run_sigmask_threads_test(void) {
    enum { NUM_WORKERS = 4 };
    pthread_t threads[NUM_WORKERS];
    struct worker_arg args[NUM_WORKERS];
    pthread_barrier_t barrier;

    g_expected_exe = expected_exe_path();
    if (pthread_barrier_init(&barrier, NULL, NUM_WORKERS) != 0) {
        fail("pthread_barrier_init");
    }
    for (int i = 0; i < NUM_WORKERS; i++) {
        args[i].id = i;
        args[i].block_sigsys = (i % 2 == 0) ? 1 : 0;
        args[i].barrier = &barrier;
        if (pthread_create(&threads[i], NULL, worker_thread_fn, &args[i]) != 0) {
            fail("pthread_create worker");
        }
    }
    for (int i = 0; i < NUM_WORKERS; i++) {
        if (pthread_join(threads[i], NULL) != 0) {
            fail("pthread_join worker");
        }
    }
    pthread_barrier_destroy(&barrier);

    sigset_t cur;
    if (pthread_sigmask(SIG_SETMASK, NULL, &cur) != 0 || sigismember(&cur, SIGSYS) != 0) {
        fail("main thread SIGSYS mask polluted");
    }

    write_all(STDOUT_FILENO, "sigmask-threads-ok\n", 19);
    return 0;
}

/* ---- proc-exe subcommand ---- */

struct sibling_tid_arg {
    pid_t tid;
    pthread_barrier_t *barrier;
};

static void *sibling_tid_fn(void *raw) {
    struct sibling_tid_arg *arg = (struct sibling_tid_arg *)raw;
    arg->tid = current_tid();
    pthread_barrier_wait(arg->barrier);
    pthread_barrier_wait(arg->barrier);
    return NULL;
}

static void assert_open_matches_file(int fd, const struct stat *expected_st, const unsigned char expected_hdr[64]) {
    if (fd < 0) {
        fail("open proc exe failed");
    }
    struct stat st;
    if (fstat(fd, &st) != 0) {
        fail("fstat opened proc exe");
    }
    if (st.st_dev != expected_st->st_dev || st.st_ino != expected_st->st_ino || st.st_size != expected_st->st_size) {
        fail("opened proc exe stat mismatch");
    }
    unsigned char hdr[64];
    ssize_t n = read(fd, hdr, sizeof hdr);
    if (n != (ssize_t)sizeof hdr || memcmp(hdr, expected_hdr, sizeof hdr) != 0) {
        fail("opened proc exe bytes mismatch");
    }
    close(fd);
}

static int run_proc_exe_test(void) {
    const char *expected = expected_exe_path();
    size_t expected_len = strlen(expected);
    pid_t pid = getpid();
    pid_t tid = current_tid();

    struct stat expected_st;
    if (stat(expected, &expected_st) != 0) {
        fail("stat EXPECTED_EXE");
    }
    unsigned char expected_hdr[64];
    {
        int efd = open(expected, O_RDONLY | O_CLOEXEC);
        if (efd < 0 || read(efd, expected_hdr, sizeof expected_hdr) != (ssize_t)sizeof expected_hdr) {
            fail("read EXPECTED_EXE header");
        }
        close(efd);
    }

    /* 1. Absolute path variants (including sibling thread TID) */
    pthread_barrier_t barrier;
    pthread_barrier_init(&barrier, NULL, 2);
    struct sibling_tid_arg sarg = {0, &barrier};
    pthread_t sib;
    if (pthread_create(&sib, NULL, sibling_tid_fn, &sarg) != 0) {
        fail("pthread_create sibling");
    }
    pthread_barrier_wait(&barrier);

    char pid_exe[128];
    char task_self_exe[128];
    char task_sib_exe[128];
    char self_task_sib_exe[128];
    snprintf(pid_exe, sizeof pid_exe, "/proc/%d/exe", (int)pid);
    snprintf(task_self_exe, sizeof task_self_exe, "/proc/%d/task/%d/exe", (int)pid, (int)tid);
    snprintf(task_sib_exe, sizeof task_sib_exe, "/proc/%d/task/%d/exe", (int)pid, (int)sarg.tid);
    snprintf(self_task_sib_exe, sizeof self_task_sib_exe, "/proc/self/task/%d/exe", (int)sarg.tid);

    const char *variants[] = {
        "/proc/self/exe",
        "/proc/thread-self/exe",
        pid_exe,
        task_self_exe,
        task_sib_exe,
        self_task_sib_exe,
        "//proc///self/./exe",
        "/proc/./self/./exe",
    };

    for (size_t i = 0; i < sizeof variants / sizeof variants[0]; i++) {
        const char *p = variants[i];
        assert_readlink_matches_expected(p, expected);

        assert_open_matches_file(openat(AT_FDCWD, p, O_RDONLY | O_CLOEXEC), &expected_st, expected_hdr);
#ifdef SYS_open
        assert_open_matches_file((int)syscall(SYS_open, p, O_RDONLY | O_CLOEXEC, 0), &expected_st, expected_hdr);
#endif
#ifdef SYS_openat2
        {
            struct raw_open_how how = {O_RDONLY | O_CLOEXEC, 0, 0};
            long fd = syscall(SYS_openat2, AT_FDCWD, p, &how, sizeof how);
            if (fd < 0 && errno == ENOSYS) {
                /* Older kernel; skip cleanly. */
            } else {
                assert_open_matches_file((int)fd, &expected_st, expected_hdr);
            }
        }
#endif

        struct stat st;
        if (stat(p, &st) != 0 || !S_ISREG(st.st_mode) || st.st_ino != expected_st.st_ino || st.st_size != expected_st.st_size) {
            fail("stat follow mismatch");
        }
        if (fstatat(AT_FDCWD, p, &st, 0) != 0 || st.st_ino != expected_st.st_ino) {
            fail("fstatat follow mismatch");
        }

        struct stat lst;
        if (lstat(p, &lst) != 0 || !S_ISLNK(lst.st_mode)) {
            fail("lstat did not report S_IFLNK");
        }
        if (fstatat(AT_FDCWD, p, &lst, AT_SYMLINK_NOFOLLOW) != 0 || !S_ISLNK(lst.st_mode)) {
            fail("fstatat NOFOLLOW did not report S_IFLNK");
        }

#ifdef SYS_statx
        {
            struct raw_statx stx;
            memset(&stx, 0, sizeof stx);
            if (syscall(SYS_statx, AT_FDCWD, p, 0, 0x7ffU, &stx) != 0) {
                fail("statx follow");
            }
            if (!S_ISREG(stx.stx_mode) || stx.stx_ino != expected_st.st_ino || stx.stx_size != (uint64_t)expected_st.st_size) {
                fail("statx follow mismatch");
            }
            memset(&stx, 0, sizeof stx);
            if (syscall(SYS_statx, AT_FDCWD, p, AT_SYMLINK_NOFOLLOW, 0x7ffU, &stx) != 0) {
                fail("statx nofollow");
            }
            if (!S_ISLNK(stx.stx_mode)) {
                fail("statx nofollow did not report S_IFLNK");
            }
        }
#endif

        if (access(p, R_OK | X_OK) != 0) {
            fail("access");
        }
        if (faccessat(AT_FDCWD, p, R_OK, 0) != 0) {
            fail("faccessat");
        }
#ifdef SYS_faccessat2
        {
            long rc = syscall(SYS_faccessat2, AT_FDCWD, p, R_OK, 0);
            if (rc != 0 && errno != ENOSYS) {
                fail("faccessat2");
            }
        }
#endif
    }

    pthread_barrier_wait(&barrier);
    pthread_join(sib, NULL);
    pthread_barrier_destroy(&barrier);

    /* 2. readlink / readlinkat edge cases: no NUL, truncation, bufsiz <= 0 */
    {
        unsigned char buf[PATH_MAX];
        memset(buf, 0xAA, sizeof buf);
        ssize_t n = readlinkat(AT_FDCWD, "/proc/self/exe", (char *)buf, sizeof buf);
        if (n != (ssize_t)expected_len) {
            fail("readlinkat full length");
        }
        if (memcmp(buf, expected, expected_len) != 0 || buf[expected_len] != 0xAA) {
            fail("readlinkat content or NUL overwrite");
        }

        memset(buf, 0xBB, 16);
        n = readlinkat(AT_FDCWD, "/proc/self/exe", (char *)buf, 5);
        if (n != 5 || memcmp(buf, expected, 5) != 0 || buf[5] != 0xBB) {
            fail("readlinkat truncation to 5");
        }

        memset(buf, 0xCC, 16);
        n = readlinkat(AT_FDCWD, "/proc/self/exe", (char *)buf, 1);
        if (n != 1 || buf[0] != (unsigned char)expected[0] || buf[1] != 0xCC) {
            fail("readlinkat truncation to 1");
        }

        errno = 0;
        if (syscall(SYS_readlinkat, AT_FDCWD, "/proc/self/exe", buf, 0) != -1 || errno != EINVAL) {
            fail("readlinkat bufsiz=0 did not return EINVAL");
        }
        errno = 0;
        if (syscall(SYS_readlinkat, AT_FDCWD, "/proc/self/exe", buf, -1) != -1 || errno != EINVAL) {
            fail("readlinkat bufsiz=-1 did not return EINVAL");
        }
#ifdef SYS_readlink
        memset(buf, 0xDD, 16);
        n = (ssize_t)syscall(SYS_readlink, "/proc/self/exe", buf, 4);
        if (n != 4 || memcmp(buf, expected, 4) != 0 || buf[4] != 0xDD) {
            fail("SYS_readlink truncation");
        }
        errno = 0;
        if (syscall(SYS_readlink, "/proc/self/exe", buf, 0) != -1 || errno != EINVAL) {
            fail("SYS_readlink bufsiz=0");
        }
#endif
    }

    /* 3. openat / openat2 / faccessat2 / execveat no-follow & resolve flags */
    errno = 0;
    if (openat(AT_FDCWD, "/proc/self/exe", O_RDONLY | O_NOFOLLOW) != -1 || errno != ELOOP) {
        fail("openat O_NOFOLLOW did not return ELOOP");
    }
    {
        int pfd = openat(AT_FDCWD, "/proc/self/exe", O_PATH | O_NOFOLLOW | O_CLOEXEC);
        if (pfd < 0) {
            fail("openat O_PATH | O_NOFOLLOW failed");
        }
        struct stat pst;
        if (fstat(pfd, &pst) != 0 || !S_ISLNK(pst.st_mode)) {
            close(pfd);
            fail("openat O_PATH | O_NOFOLLOW did not open symlink itself");
        }
        close(pfd);
    }
#ifdef SYS_openat2
    {
        struct raw_open_how how_nf = {O_RDONLY | O_NOFOLLOW | O_CLOEXEC, 0, 0};
        errno = 0;
        long rc = syscall(SYS_openat2, AT_FDCWD, "/proc/self/exe", &how_nf, sizeof how_nf);
        if (rc != -1 || (errno != ELOOP && errno != ENOSYS)) {
            fail("openat2 O_NOFOLLOW did not return ELOOP");
        }
        if (errno != ENOSYS) {
            struct raw_open_how how_nosym = {O_RDONLY | O_CLOEXEC, 0, RESOLVE_NO_SYMLINKS};
            errno = 0;
            if (syscall(SYS_openat2, AT_FDCWD, "/proc/self/exe", &how_nosym, sizeof how_nosym) != -1
                || errno != ELOOP) {
                fail("openat2 RESOLVE_NO_SYMLINKS did not return ELOOP");
            }
            struct raw_open_how how_nomagic = {O_RDONLY | O_CLOEXEC, 0, RESOLVE_NO_MAGICLINKS};
            errno = 0;
            if (syscall(SYS_openat2, AT_FDCWD, "/proc/self/exe", &how_nomagic, sizeof how_nomagic) != -1
                || errno != ELOOP) {
                fail("openat2 RESOLVE_NO_MAGICLINKS did not return ELOOP");
            }
            struct raw_open_how how_beneath = {O_RDONLY | O_CLOEXEC, 0, RESOLVE_BENEATH};
            errno = 0;
            if (syscall(SYS_openat2, AT_FDCWD, "/proc/self/exe", &how_beneath, sizeof how_beneath) != -1
                || errno != EXDEV) {
                fail("openat2 RESOLVE_BENEATH did not return EXDEV");
            }
        }
    }
#endif
#ifdef SYS_faccessat2
    {
        errno = 0;
        long rc = syscall(SYS_faccessat2, AT_FDCWD, "/proc/self/exe", F_OK, AT_SYMLINK_NOFOLLOW);
        if (rc != 0 && errno != ENOSYS) {
            fail("faccessat2 AT_SYMLINK_NOFOLLOW failed");
        }
    }
#endif
#ifdef SYS_execveat
    {
        char *dummy_argv[] = {(char *)"exe", NULL};
        errno = 0;
        long rc = syscall(SYS_execveat, AT_FDCWD, "/proc/self/exe", dummy_argv, environ, AT_SYMLINK_NOFOLLOW);
        if (rc != -1 || errno != ELOOP) {
            fail("execveat AT_SYMLINK_NOFOLLOW did not return ELOOP");
        }
    }
#endif

    /* 4. Relative lookups via dirfd (/proc/self, /proc, /proc/<pid>) */
    {
        int d_self = open("/proc/self", O_RDONLY | O_DIRECTORY | O_CLOEXEC);
        int d_proc = open("/proc", O_RDONLY | O_DIRECTORY | O_CLOEXEC);
        char pid_dir[64];
        snprintf(pid_dir, sizeof pid_dir, "/proc/%d", (int)pid);
        int d_pid = open(pid_dir, O_RDONLY | O_DIRECTORY | O_CLOEXEC);
        if (d_self < 0 || d_proc < 0 || d_pid < 0) {
            fail("open proc dirfds");
        }

        char buf[PATH_MAX];
        ssize_t n = readlinkat(d_self, "exe", buf, sizeof buf - 1);
        if (n < 0 || (buf[n] = '\0', strcmp(buf, expected) != 0)) {
            fail("readlinkat(d_self, exe)");
        }
        n = readlinkat(d_proc, "self/exe", buf, sizeof buf - 1);
        if (n < 0 || (buf[n] = '\0', strcmp(buf, expected) != 0)) {
            fail("readlinkat(d_proc, self/exe)");
        }
        n = readlinkat(d_pid, "exe", buf, sizeof buf - 1);
        if (n < 0 || (buf[n] = '\0', strcmp(buf, expected) != 0)) {
            fail("readlinkat(d_pid, exe)");
        }

        assert_open_matches_file(openat(d_self, "exe", O_RDONLY | O_CLOEXEC), &expected_st, expected_hdr);
        struct stat st;
        if (fstatat(d_self, "exe", &st, 0) != 0 || st.st_ino != expected_st.st_ino) {
            fail("fstatat(d_self, exe)");
        }
        if (faccessat(d_self, "exe", R_OK, 0) != 0) {
            fail("faccessat(d_self, exe)");
        }

        close(d_self);
        close(d_proc);
        close(d_pid);
    }

    /* 5. Relative lookups via cwd = /proc/self */
    {
        char saved_cwd[PATH_MAX];
        if (getcwd(saved_cwd, sizeof saved_cwd) == NULL) {
            fail("getcwd");
        }
        if (chdir("/proc/self") != 0) {
            fail("chdir /proc/self");
        }
        assert_readlink_matches_expected("exe", expected);
        assert_open_matches_file(open("exe", O_RDONLY | O_CLOEXEC), &expected_st, expected_hdr);
        struct stat st;
        if (stat("exe", &st) != 0 || st.st_ino != expected_st.st_ino) {
            fail("stat cwd exe");
        }
        if (chdir(saved_cwd) != 0) {
            fail("restore cwd");
        }
    }

    /* 6. realpath("/proc/self/exe") matches expected canonical path */
    {
        char resolved[PATH_MAX];
        if (realpath("/proc/self/exe", resolved) == NULL || strcmp(resolved, expected) != 0) {
            fail("realpath(/proc/self/exe)");
        }
    }

    /* 7. Non-matching paths, non-/proc dirfd, invalid dirfd, lexical `..`, and /proc/<other_pid>/exe */
    {
        const char *other_exe = getenv("OTHER_EXE_LINK");
        if (other_exe != NULL) {
            char buf[PATH_MAX];
            ssize_t n = readlink(other_exe, buf, sizeof buf - 1);
            if (n < 0 || (buf[n] = '\0', strcmp(buf, "/dev/null") != 0)) {
                fail("unrelated exe symlink was wrongly virtualized");
            }
            const char *other_dir = getenv("OTHER_EXE_DIR");
            if (other_dir != NULL) {
                int d_other = open(other_dir, O_RDONLY | O_DIRECTORY | O_CLOEXEC);
                if (d_other < 0) {
                    fail("open OTHER_EXE_DIR");
                }
                n = readlinkat(d_other, "exe", buf, sizeof buf - 1);
                close(d_other);
                if (n < 0 || (buf[n] = '\0', strcmp(buf, "/dev/null") != 0)) {
                    fail("non-/proc dirfd + exe was wrongly virtualized");
                }
            }
            const char *other_sub = getenv("OTHER_EXE_SUBDIR");
            if (other_sub != NULL) {
                char saved_cwd[PATH_MAX];
                if (getcwd(saved_cwd, sizeof saved_cwd) == NULL) {
                    fail("getcwd before dotdot");
                }
                if (chdir(other_sub) != 0) {
                    fail("chdir OTHER_EXE_SUBDIR");
                }
                /* `/proc/self/cwd/../exe` resolves through the cwd symlink to
                 * OTHER_EXE_LINK (/dev/null), NOT `/proc/self/exe`. */
                n = readlink("/proc/self/cwd/../exe", buf, sizeof buf - 1);
                int saved_err = errno;
                (void)chdir(saved_cwd);
                if (n < 0) {
                    errno = saved_err;
                    fail("readlink /proc/self/cwd/../exe");
                }
                buf[n] = '\0';
                if (strcmp(buf, "/dev/null") != 0) {
                    fail("lexical .. in /proc/self/cwd/../exe was falsely rewritten");
                }
            }
        }
        /* Nonexistent prefix + `..` must return ENOENT, never falsely rewritten. */
        struct stat st_bad;
        errno = 0;
        if (stat("/proc/nonexistent_fspy_dir/../self/exe", &st_bad) != -1 || errno != ENOENT) {
            fail("lexical .. in /proc/nonexistent/../self/exe was falsely rewritten");
        }
        /* Invalid dirfds with relative "exe" must fail with EBADF. */
        char tmpbuf[64];
        errno = 0;
        if (readlinkat(-1, "exe", tmpbuf, sizeof tmpbuf) != -1 || errno != EBADF) {
            fail("readlinkat(-1, exe) did not return EBADF");
        }
        errno = 0;
        if (readlinkat(99999, "exe", tmpbuf, sizeof tmpbuf) != -1 || errno != EBADF) {
            fail("readlinkat(99999, exe) did not return EBADF");
        }
        /* Another process's `/proc/<other_pid>/exe` must NOT be rewritten. */
        const char *other_pid_str = getenv("OTHER_PID");
        const char *other_pid_exe = getenv("OTHER_PID_EXE");
        if (other_pid_str != NULL && other_pid_exe != NULL) {
            char other_proc_exe[64];
            snprintf(other_proc_exe, sizeof other_proc_exe, "/proc/%s/exe", other_pid_str);
            char buf[PATH_MAX];
            ssize_t n = readlink(other_proc_exe, buf, sizeof buf - 1);
            if (n < 0) {
                fail("readlink /proc/<other_pid>/exe");
            }
            buf[n] = '\0';
            if (strcmp(buf, other_pid_exe) != 0) {
                fail("/proc/<other_pid>/exe was wrongly rewritten");
            }
        }
    }

    /* 8. Standalone execve("/proc/self/exe") & execveat(AT_FDCWD, "/proc/self/exe")
     * without readlink or a caller interceptor (static image has no ld-linux
     * trapping openat before main; on static glibc, glibc's own startup CRT
     * executes trapped syscalls outside the launcher PF_X range before main,
     * which kills the child with SIGSYS instead of running the launcher's
     * panic exit 101). */
    if (getauxval(AT_BASE) == 0) {
        for (int use_at = 0; use_at < 2; use_at++) {
            pid_t child = fork();
            if (child < 0) {
                fail("fork for /proc/self/exe execve");
            }
            if (child == 0) {
                char *reexec_argv[] = {(char *)"reexec-self", (char *)"reexec-exit-73", NULL};
                if (use_at == 0) {
                    execve("/proc/self/exe", reexec_argv, environ);
                } else {
#ifdef SYS_execveat
                    syscall(SYS_execveat, AT_FDCWD, "/proc/self/exe", reexec_argv, environ, 0);
#else
                    _exit(73);
#endif
                }
                _exit(99);
            }
            int status = 0;
            if (waitpid(child, &status, 0) != child) {
                fail("waitpid for standalone execve(/proc/self/exe)");
            }
#if defined(__GLIBC__)
            int ok = (WIFEXITED(status) && WEXITSTATUS(status) == 73)
                || (WIFSIGNALED(status) && WTERMSIG(status) == SIGSYS);
#else
            int ok = WIFEXITED(status) && WEXITSTATUS(status) == 73;
#endif
            if (!ok) {
                fprintf(
                    stderr,
                    "FAIL: standalone execve(/proc/self/exe) unexpected status=0x%x (use_at=%d)\n",
                    status,
                    use_at
                );
                _exit(2);
            }
        }
    }

    write_all(STDOUT_FILENO, "proc-exe-ok\n", 12);
    return 0;
}

/* ---- comm subcommand (PR_SET_NAME) ---- */

static int run_comm_test(void) {
    const char *expected = getenv("EXPECTED_COMM");
    if (expected == NULL) {
        fail("missing EXPECTED_COMM");
    }
    int fd = open("/proc/self/comm", O_RDONLY | O_CLOEXEC);
    if (fd < 0) {
        fail("open /proc/self/comm");
    }
    char buf[64];
    ssize_t n = read(fd, buf, sizeof buf - 1);
    close(fd);
    if (n <= 0) {
        fail("read /proc/self/comm");
    }
    while (n > 0 && buf[n - 1] == '\n') {
        n--;
    }
    buf[n] = '\0';
    if (strcmp(buf, expected) != 0) {
        char msg[256];
        snprintf(msg, sizeof msg, "/proc/self/comm mismatch: got '%s', expected '%s'", buf, expected);
        fail(msg);
    }
    write_all(STDOUT_FILENO, "comm-ok\n", 8);
    return 0;
}

/* ---- deep-stack subcommand (RLIMIT_STACK > 8 MiB) ---- */

__attribute__((noinline))
static uint64_t touch_deep_stack(int depth) {
    volatile unsigned char frame[65536];
    frame[0] = (unsigned char)(depth & 0xff);
    frame[sizeof frame - 1] = (unsigned char)((depth * 7) & 0xff);
    uint64_t acc = (uint64_t)frame[0] + (uint64_t)frame[sizeof frame - 1];
    if (depth > 1) {
        acc += touch_deep_stack(depth - 1);
    }
    return acc;
}

static int run_deep_stack_test(void) {
    /* 160 frames * 64 KiB = 10 MiB (> default 8 MiB fixed stack, fits in 16 MiB). */
    uint64_t sum = touch_deep_stack(160);
    if (sum == 0) {
        fail("unexpected zero deep-stack checksum");
    }
    write_all(STDOUT_FILENO, "deep-stack-ok\n", 14);
    return 0;
}

/* ---- interceptor subcommand ---- */

static int execve_fail_vfork_child_fn(void *arg) {
    volatile uint32_t *counter = (volatile uint32_t *)arg;
    errno = 0;
    if (syscall(SYS_execve, "/bin/true", (void *)0x55552345UL, (void *)counter) != -1
        || errno != ENOENT) {
        _exit(61);
    }
    if (*counter != 1) {
        _exit(62);
    }
    errno = 0;
    if (syscall(SYS_execve, "/bin/true", (void *)0x55552346UL, NULL) != -1 || errno != ENOENT) {
        _exit(63);
    }
    if (*counter != 1) {
        _exit(64);
    }
    _exit(0);
}

static int run_interceptor_test(void) {
    const char *expected = expected_exe_path();
    assert_readlink_matches_expected("/proc/self/exe", expected);

    /* 1. Ok(()) replays custom trapped syscall `getegid`. */
    gid_t egid_normal = (gid_t)syscall(SYS_getegid, 0UL);
    if (egid_normal != getegid()) {
        fail("intercepted getegid replay mismatch");
    }

    /* 2. Err(ACCES) short-circuits with -EACCES without replaying. */
    errno = 0;
    if (syscall(SYS_getegid, 0xdeadbeefUL) != -1 || errno != EACCES) {
        fail("intercepted getegid short-circuit did not return EACCES");
    }

    /* 2b. SyscallAction::Return(rc) short-circuits in Phase 1 with exact rc. */
    if (syscall(SYS_getegid, 0xdead0000UL) != 0) {
        fail("SyscallAction::Return(0) did not return 0");
    }
    if (syscall(SYS_getegid, 0xdead0042UL) != 42) {
        fail("SyscallAction::Return(42) did not return 42");
    }
    if (syscall(SYS_faccessat, AT_FDCWD, "/dev/null/fspy_return_probe", 0x7778UL) != 0) {
        fail("SyscallAction::Return(0) on faccessat did not short-circuit");
    }

    /* 3. /proc/self/exe rewrite is visible to interceptor on faccessat. */
    errno = 0;
    if (syscall(SYS_faccessat, AT_FDCWD, "/proc/self/exe", 0x7777UL) != -1 || errno != ELOOP) {
        fail("interceptor did not observe rewritten /proc/self/exe on faccessat");
    }
    if (syscall(SYS_faccessat, AT_FDCWD, "/proc/self/exe", R_OK) != 0) {
        fail("normal faccessat(/proc/self/exe) failed under interceptor");
    }

    /* 4. /proc/self/exe rewrite is visible to interceptor on execve & execveat. */
    errno = 0;
    if (syscall(SYS_execve, "/proc/self/exe", (void *)0x55551234UL, NULL) != -1 || errno != ELOOP) {
        fail("interceptor did not observe rewritten /proc/self/exe on execve");
    }
    errno = 0;
    if (syscall(SYS_execveat, AT_FDCWD, "/proc/self/exe", (void *)0x55551234UL, NULL, 0) != -1 || errno != ELOOP) {
        fail("interceptor did not observe rewritten /proc/self/exe on execveat");
    }
    int d_self = open("/proc/self", O_RDONLY | O_DIRECTORY | O_CLOEXEC);
    if (d_self < 0) {
        fail("open /proc/self for execveat");
    }
    errno = 0;
    if (syscall(SYS_execveat, d_self, "exe", (void *)0x55551234UL, NULL, 0) != -1 || errno != ELOOP) {
        close(d_self);
        fail("interceptor did not observe rewritten dirfd /proc/self/exe on execveat");
    }
    close(d_self);

    /* 5. SyscallAction::Execve failure path (with cleanup callback and with
     * cleanup: None), invoked both with SIGSYS virtually blocked and from a
     * vfork child. */
    {
        sigset_t only_sigsys, cur;
        sigemptyset(&only_sigsys);
        sigaddset(&only_sigsys, SIGSYS);
        if (sigprocmask(SIG_BLOCK, &only_sigsys, NULL) != 0) {
            fail("interceptor block SIGSYS");
        }

        volatile uint32_t cleanup_count = 0;
        errno = 0;
        if (syscall(SYS_execve, "/bin/true", (void *)0x55552345UL, (void *)&cleanup_count) != -1
            || errno != ENOENT) {
            fail("SyscallAction::Execve with cleanup did not fail with ENOENT");
        }
        if (cleanup_count != 1) {
            fail("SyscallAction::Execve cleanup callback did not run");
        }
        sigemptyset(&cur);
        if (sigprocmask(SIG_SETMASK, NULL, &cur) != 0 || sigismember(&cur, SIGSYS) != 1) {
            fail("failed SyscallAction::Execve lost blocked SIGSYS TID entry");
        }

        errno = 0;
        if (syscall(SYS_execve, "/bin/true", (void *)0x55552346UL, NULL) != -1 || errno != ENOENT) {
            fail("SyscallAction::Execve without cleanup did not fail with ENOENT");
        }
        if (cleanup_count != 1) {
            fail("SyscallAction::Execve without cleanup unexpectedly ran callback");
        }
        if (sigprocmask(SIG_UNBLOCK, &only_sigsys, NULL) != 0) {
            fail("interceptor unblock SIGSYS");
        }

        /* Also exercise inside a CLONE_VM | CLONE_VFORK child on a small stack. */
        size_t stack_sz = 16 * 1024;
        unsigned char *stack_mem =
            mmap(NULL, stack_sz, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
        if (stack_mem == MAP_FAILED) {
            fail("mmap vfork execve fail stack");
        }
        void *stack_top = (void *)(((uintptr_t)(stack_mem + stack_sz)) & ~(uintptr_t)15);
        volatile uint32_t vfork_cleanup_count = 0;
        pid_t child = clone(
            execve_fail_vfork_child_fn,
            stack_top,
            CLONE_VM | CLONE_VFORK | SIGCHLD,
            (void *)&vfork_cleanup_count
        );
        if (child < 0) {
            fail("clone vfork execve fail");
        }
        int status = 0;
        if (waitpid(child, &status, 0) != child || !WIFEXITED(status) || WEXITSTATUS(status) != 0) {
            fail("vfork SyscallAction::Execve failure child failed");
        }
        if (vfork_cleanup_count != 1) {
            fail("vfork SyscallAction::Execve cleanup callback count mismatch");
        }
        munmap(stack_mem, stack_sz);
    }

    write_all(STDOUT_FILENO, "interceptor-ok\n", 15);
    return 0;
}

/* ---- recursive-exec subcommand (no seccomp filter stacking) ---- */

static int read_seccomp_filter_count(void) {
    int fd = open("/proc/self/status", O_RDONLY | O_CLOEXEC);
    if (fd < 0) {
        fail("open /proc/self/status");
    }
    char buf[8192];
    ssize_t n = read(fd, buf, sizeof buf - 1);
    close(fd);
    if (n <= 0) {
        fail("read /proc/self/status");
    }
    buf[n] = '\0';
    const char *needle = "Seccomp_filters:\t";
    const char *p = strstr(buf, needle);
    if (p == NULL) {
        fail("Seccomp_filters field missing in /proc/self/status");
    }
    return atoi(p + strlen(needle));
}

static int run_recursive_exec_test(void) {
    const char *expected = expected_exe_path();
    const char *stage = getenv("RECURSIVE_STAGE");
    if (stage == NULL) {
        fail("missing RECURSIVE_STAGE");
    }
    int count = read_seccomp_filter_count();
    if (count < 1) {
        fail("expected at least 1 seccomp filter");
    }
    assert_readlink_matches_expected("/proc/self/exe", expected);

    if (strcmp(stage, "1") == 0) {
        const char *launcher = getenv("LAUNCHER_PATH");
        if (launcher == NULL) {
            fail("missing LAUNCHER_PATH");
        }
        char count_str[32];
        snprintf(count_str, sizeof count_str, "%d", count);
        setenv("PARENT_SECCOMP_FILTERS", count_str, 1);
        setenv("RECURSIVE_STAGE", "2", 1);

        /* Block SIGUSR2 before execv; child must inherit it blocked. */
        sigset_t block_u2;
        sigemptyset(&block_u2);
        sigaddset(&block_u2, SIGUSR2);
        if (sigprocmask(SIG_BLOCK, &block_u2, NULL) != 0) {
            fail("recursive exec block SIGUSR2");
        }

        const char *use_interceptor = getenv("USE_INTERCEPTOR");
        if (use_interceptor != NULL && strcmp(use_interceptor, "1") == 0) {
            char *argv_next[] = {
                (char *)launcher,
                (char *)"--test-interceptor",
                (char *)expected,
                (char *)"recursive-stage-2",
                (char *)"recursive-exec",
                NULL,
            };
            execv(launcher, argv_next);
        } else {
            char *argv_next[] = {
                (char *)launcher,
                (char *)expected,
                (char *)"recursive-stage-2",
                (char *)"recursive-exec",
                NULL,
            };
            execv(launcher, argv_next);
        }
        fail("recursive execv launcher failed");
    }

    const char *parent_str = getenv("PARENT_SECCOMP_FILTERS");
    if (parent_str == NULL) {
        fail("missing PARENT_SECCOMP_FILTERS");
    }
    int parent_count = atoi(parent_str);
    const char *expect_inc = getenv("EXPECT_FILTER_INCREMENT");
    int expected_count = (expect_inc != NULL && strcmp(expect_inc, "1") == 0)
                             ? (parent_count + 1)
                             : parent_count;
    if (count != expected_count) {
        char msg[128];
        snprintf(
            msg,
            sizeof msg,
            "seccomp filter count mismatch: parent=%d expected=%d child=%d",
            parent_count,
            expected_count,
            count
        );
        fail(msg);
    }
    if (expect_inc != NULL && strcmp(expect_inc, "1") == 0) {
        errno = 0;
        if (syscall(SYS_getegid, 0xdeadbeefUL) != -1 || errno != EACCES) {
            fail("new interceptor filter was not active after mismatched-fingerprint re-exec");
        }
    }

    sigset_t inherited;
    sigemptyset(&inherited);
    if (sigprocmask(SIG_SETMASK, NULL, &inherited) != 0 || sigismember(&inherited, SIGUSR2) != 1) {
        fail("recursive exec did not preserve blocked SIGUSR2");
    }

    write_all(STDOUT_FILENO, "recursive-exec-ok\n", 18);
    return 0;
}

/* ---- nested-seccomp subcommand (target's own SECCOMP_RET_TRAP filter) ---- */

static volatile sig_atomic_t g_nested_trap_count = 0;
static volatile sig_atomic_t g_nested_trap_errno = -1;
static volatile sig_atomic_t g_nested_trap_readlink_ok = 0;

static void nested_seccomp_sigsys_handler(int sig, siginfo_t *info, void *raw_ucontext) {
    if (sig != SIGSYS || info == NULL || raw_ucontext == NULL) {
        return;
    }
    if (info->si_code != SYS_SECCOMP) {
        return;
    }
    g_nested_trap_count++;
    g_nested_trap_errno = info->si_errno;

    /* Loader's own tagged SECCOMP_RET_TRAP must still work even inside the
     * target's forwarded SIGSYS handler! */
    char buf[PATH_MAX];
    ssize_t n = readlink("/proc/self/exe", buf, sizeof buf - 1);
    if (n > 0 && (size_t)n == strlen(g_expected_exe) && memcmp(buf, g_expected_exe, (size_t)n) == 0) {
        g_nested_trap_readlink_ok = 1;
    }

    ucontext_t *uc = (ucontext_t *)raw_ucontext;
#if defined(__x86_64__)
    uc->uc_mcontext.gregs[REG_RAX] = 4242;
#elif defined(__aarch64__)
    uc->uc_mcontext.regs[0] = 4242;
#else
#error "unsupported architecture"
#endif
}

static int run_nested_seccomp_test(void) {
    g_expected_exe = expected_exe_path();

    struct sigaction sa;
    memset(&sa, 0, sizeof sa);
    sa.sa_sigaction = nested_seccomp_sigsys_handler;
    sa.sa_flags = SA_SIGINFO;
    sigemptyset(&sa.sa_mask);
    if (sigaction(SIGSYS, &sa, NULL) != 0) {
        fail("nested sigaction(SIGSYS)");
    }

    if (prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0) {
        fail("nested prctl(PR_SET_NO_NEW_PRIVS)");
    }

    /* BPF filter: if nr == SYS_getuid -> SECCOMP_RET_TRAP (data=0), else ALLOW. */
    struct raw_sock_filter insns[] = {
        {0x20, 0, 0, 0},                        /* LD_W_ABS offsetof(seccomp_data, nr) */
        {0x15, 0, 1, (uint32_t)SYS_getuid},     /* JEQ SYS_getuid -> +0 else +1 */
        {0x06, 0, 0, 0x00030000U},              /* RET SECCOMP_RET_TRAP | 0 */
        {0x06, 0, 0, 0x7fff0000U},              /* RET SECCOMP_RET_ALLOW */
    };
    struct raw_sock_fprog prog = {
        .len = (uint16_t)(sizeof insns / sizeof insns[0]),
        .filter = insns,
    };
    if (syscall(SYS_seccomp, 1 /* SECCOMP_SET_MODE_FILTER */, 0, &prog) != 0) {
        fail("nested seccomp(SECCOMP_SET_MODE_FILTER)");
    }

    long uid = syscall(SYS_getuid);
    if (uid != 4242) {
        char msg[128];
        snprintf(msg, sizeof msg, "nested seccomp getuid returned %ld instead of 4242", uid);
        fail(msg);
    }
    if (g_nested_trap_count != 1 || g_nested_trap_errno != 0 || !g_nested_trap_readlink_ok) {
        fail("nested seccomp handler state mismatch");
    }

    /* Loader's own /proc/self/exe traps must continue working afterward. */
    assert_readlink_matches_expected("/proc/self/exe", g_expected_exe);

    write_all(STDOUT_FILENO, "nested-seccomp-ok\n", 18);
    return 0;
}

/* ---- posix-spawn subcommand (clone3 CLONE_CLEAR_SIGHAND fallback) ---- */

static int run_posix_spawn_test(void) {
    posix_spawnattr_t attr;
    if (posix_spawnattr_init(&attr) != 0) {
        fail("posix_spawnattr_init");
    }
    sigset_t full;
    sigfillset(&full);
    if (posix_spawnattr_setflags(&attr, POSIX_SPAWN_SETSIGDEF | POSIX_SPAWN_SETSIGMASK) != 0) {
        fail("posix_spawnattr_setflags");
    }
    if (posix_spawnattr_setsigdefault(&attr, &full) != 0) {
        fail("posix_spawnattr_setsigdefault");
    }
    if (posix_spawnattr_setsigmask(&attr, &full) != 0) {
        fail("posix_spawnattr_setsigmask");
    }

    /* 1. Failed exec in posix_spawn must propagate ENOENT instead of killing
     * the vfork child with SIGSYS (which leaves glibc's shared errno 0). */
    {
        char *argv_missing[] = {(char *)"/nonexistent_fspy_posix_spawn_probe", NULL};
        pid_t missing_pid = -1;
        int missing_rc = posix_spawn(
            &missing_pid,
            "/nonexistent_fspy_posix_spawn_probe",
            NULL,
            &attr,
            argv_missing,
            environ
        );
        if (missing_rc != ENOENT) {
            char msg[128];
            snprintf(msg, sizeof msg, "posix_spawn missing expected ENOENT, got %d", missing_rc);
            fail(msg);
        }
    }

    /* 2. Successful posix_spawn through the launcher (or direct kernel exec). */
    const char *launcher = getenv("LAUNCHER_PATH");
    pid_t pid = -1;
    int rc;
    if (launcher != NULL && launcher[0] != '\0') {
        char *argv_child[] = {(char *)launcher, (char *)"/bin/true", (char *)"true", NULL};
        rc = posix_spawn(&pid, launcher, NULL, &attr, argv_child, environ);
    } else {
        char *argv_child[] = {(char *)"/bin/true", NULL};
        rc = posix_spawn(&pid, "/bin/true", NULL, &attr, argv_child, environ);
    }
    posix_spawnattr_destroy(&attr);
    if (rc != 0) {
        errno = rc;
        fail("posix_spawn /bin/true");
    }

    int status = 0;
    if (waitpid(pid, &status, 0) != pid) {
        fail("waitpid posix_spawn child");
    }
    if (!WIFEXITED(status) || WEXITSTATUS(status) != 0) {
        char msg[128];
        snprintf(msg, sizeof msg, "posix_spawn child exited abnormally: status=0x%x", status);
        fail(msg);
    }

    write_all(STDOUT_FILENO, "posix-spawn-ok\n", 15);
    return 0;
}

static int run_clone3_enosys_test(void) {
#ifdef SYS_clone3
    uint64_t args[11];
    memset(args, 0, sizeof args);
    /* CLONE_CLEAR_SIGHAND = 0x100000000ULL */
    args[0] = UINT64_C(0x100000000);
    errno = 0;
    long rc = syscall(SYS_clone3, args, sizeof args);
    if (rc != -1 || errno != ENOSYS) {
        char msg[128];
        snprintf(msg, sizeof msg, "clone3 expected -1/ENOSYS, got rc=%ld errno=%d", rc, errno);
        fail(msg);
    }
#endif
    write_all(STDOUT_FILENO, "clone3-enosys-ok\n", 17);
    return 0;
}

/* ---- sigaltstack-storm subcommand (Go-style SA_ONSTACK SIGURG stress) ---- */

static volatile sig_atomic_t g_storm_stop = 0;
static volatile sig_atomic_t g_storm_sig_count = 0;

static void storm_sigusr1_handler(int sig) {
    if (sig == SIGUSR1) {
        g_storm_sig_count++;
    }
}

struct storm_arg {
    pthread_t target;
};

static void *storm_sender_thread(void *raw) {
    struct storm_arg *arg = (struct storm_arg *)raw;
    while (!g_storm_stop) {
        pthread_kill(arg->target, SIGUSR1);
        struct timespec ts = {0, 50 * 1000}; /* 50 us */
        nanosleep(&ts, NULL);
    }
    return NULL;
}

static int run_sigaltstack_storm_test(void) {
    const char *expected = expected_exe_path();
    static unsigned char alt_stack_mem[64 * 1024];
    stack_t ss;
    ss.ss_sp = alt_stack_mem;
    ss.ss_size = sizeof alt_stack_mem;
    ss.ss_flags = 0;
    if (sigaltstack(&ss, NULL) != 0) {
        fail("sigaltstack");
    }

    struct sigaction sa;
    memset(&sa, 0, sizeof sa);
    sa.sa_handler = storm_sigusr1_handler;
    sa.sa_flags = SA_ONSTACK | SA_RESTART;
    sigemptyset(&sa.sa_mask);
    if (sigaction(SIGUSR1, &sa, NULL) != 0) {
        fail("sigaction SIGUSR1 storm");
    }

    g_storm_stop = 0;
    g_storm_sig_count = 0;
    struct storm_arg sarg = {pthread_self()};
    pthread_t sender;
    if (pthread_create(&sender, NULL, storm_sender_thread, &sarg) != 0) {
        fail("pthread_create storm sender");
    }

    for (int i = 0; i < 3000; i++) {
        int fd = openat(AT_FDCWD, "/dev/null", O_RDONLY | O_CLOEXEC);
        if (fd < 0) {
            fail("storm openat /dev/null");
        }
        close(fd);

        struct stat st;
        if (fstatat(AT_FDCWD, "/dev/null", &st, 0) != 0) {
            fail("storm fstatat /dev/null");
        }

        assert_readlink_matches_expected("/proc/self/exe", expected);

        struct timespec zero = {0, 0};
        if (ppoll(NULL, 0, &zero, NULL) < 0 && errno != EINTR) {
            fail("storm ppoll(NULL)");
        }

        sigset_t u2;
        sigemptyset(&u2);
        sigaddset(&u2, SIGUSR2);
        if (sigprocmask(SIG_BLOCK, &u2, NULL) != 0) {
            fail("storm sigprocmask block");
        }
        if (sigprocmask(SIG_UNBLOCK, &u2, NULL) != 0) {
            fail("storm sigprocmask unblock");
        }
    }

    g_storm_stop = 1;
    pthread_join(sender, NULL);
    if (g_storm_sig_count == 0) {
        fail("storm received zero SIGUSR1 signals");
    }

    write_all(STDOUT_FILENO, "sigaltstack-storm-ok\n", 21);
    return 0;
}

/* ---- fifo-open-eintr subcommand (blocking open interrupted by signal) ---- */

static volatile sig_atomic_t g_fifo_done = 0;
static volatile sig_atomic_t g_fifo_sig_count = 0;

static void fifo_sigusr1_handler(int sig) {
    if (sig == SIGUSR1) {
        g_fifo_sig_count++;
    }
}

static void *fifo_sender_thread(void *raw) {
    struct storm_arg *arg = (struct storm_arg *)raw;
    for (int i = 0; i < 400 && !g_fifo_done; i++) {
        struct timespec ts = {0, 5 * 1000 * 1000}; /* 5 ms */
        nanosleep(&ts, NULL);
        if (!g_fifo_done) {
            pthread_kill(arg->target, SIGUSR1);
        }
    }
    return NULL;
}

static int run_fifo_open_eintr_test(void) {
    const char *fifo_path = getenv("FIFO_PATH");
    if (fifo_path == NULL || fifo_path[0] == '\0') {
        fail("missing FIFO_PATH");
    }
    if (mkfifo(fifo_path, 0600) != 0) {
        fail("mkfifo");
    }

    struct sigaction sa;
    memset(&sa, 0, sizeof sa);
    sa.sa_handler = fifo_sigusr1_handler;
    sa.sa_flags = 0; /* no SA_RESTART: blocking open must fail with EINTR */
    sigemptyset(&sa.sa_mask);
    if (sigaction(SIGUSR1, &sa, NULL) != 0) {
        fail("sigaction SIGUSR1 fifo");
    }

    g_fifo_done = 0;
    g_fifo_sig_count = 0;
    struct storm_arg sarg = {pthread_self()};
    pthread_t sender;
    if (pthread_create(&sender, NULL, fifo_sender_thread, &sarg) != 0) {
        fail("pthread_create fifo sender");
    }

    errno = 0;
    int fd = open(fifo_path, O_RDONLY);
    int open_err = errno;
    g_fifo_done = 1;
    pthread_join(sender, NULL);
    if (fd >= 0) {
        close(fd);
    }

    if (fd != -1 || open_err != EINTR || g_fifo_sig_count == 0) {
        unlink(fifo_path);
        char msg[128];
        snprintf(msg, sizeof msg, "blocking open(fifo) expected -1/EINTR, got fd=%d errno=%d sigs=%d",
                 fd, open_err, (int)g_fifo_sig_count);
        fail(msg);
    }

#ifdef SYS_openat2
    /* Also verify unmasked blocking `openat2(fifo)` is interruptible with EINTR. */
    g_fifo_done = 0;
    g_fifo_sig_count = 0;
    if (pthread_create(&sender, NULL, fifo_sender_thread, &sarg) != 0) {
        unlink(fifo_path);
        fail("pthread_create fifo openat2 sender");
    }
    struct raw_open_how how = {O_RDONLY, 0, 0};
    errno = 0;
    long fd2 = syscall(SYS_openat2, AT_FDCWD, fifo_path, &how, sizeof how);
    int open2_err = errno;
    g_fifo_done = 1;
    pthread_join(sender, NULL);
    if (fd2 >= 0) {
        close((int)fd2);
    }
    unlink(fifo_path);
    if (!(fd2 == -1 && open2_err == ENOSYS)) {
        if (fd2 != -1 || open2_err != EINTR || g_fifo_sig_count == 0) {
            char msg[128];
            snprintf(
                msg,
                sizeof msg,
                "blocking openat2(fifo) expected -1/EINTR, got fd=%ld errno=%d sigs=%d",
                fd2,
                open2_err,
                (int)g_fifo_sig_count
            );
            fail(msg);
        }
    }
#else
    unlink(fifo_path);
#endif

    write_all(STDOUT_FILENO, "fifo-open-eintr-ok\n", 19);
    return 0;
}

/* ---- blocked-sigsys-pending subcommand ---- */

static volatile sig_atomic_t g_queued_sigsys_count = 0;
static volatile int g_queued_sigsys_val = 0;
static volatile sig_atomic_t g_queued_usr1_count = 0;

static void sigsys_queued_info_handler(int sig, siginfo_t *info, void *ucontext) {
    (void)ucontext;
    if (sig == SIGSYS) {
        g_queued_sigsys_count++;
        g_queued_sigsys_val = info != NULL ? info->si_value.sival_int : -1;
        check_readlink_in_handler();
    }
}

static void sigusr1_queued_handler(int sig) {
    if (sig == SIGUSR1) {
        g_queued_usr1_count++;
    }
}

struct sig_ign_worker_arg {
    pthread_barrier_t *b1;
    pthread_barrier_t *b2;
    pthread_barrier_t *b3;
    pthread_barrier_t *b4;
};

static void *sig_ign_pending_worker(void *raw) {
    struct sig_ign_worker_arg *arg = (struct sig_ign_worker_arg *)raw;
    sigset_t only_sigsys, pending;
    sigemptyset(&only_sigsys);
    sigaddset(&only_sigsys, SIGSYS);
    if (pthread_sigmask(SIG_BLOCK, &only_sigsys, NULL) != 0) {
        fail("sig_ign worker block SIGSYS");
    }
    pthread_barrier_wait(arg->b1);
    pthread_barrier_wait(arg->b2);

    sigemptyset(&pending);
    if (sigpending(&pending) != 0 || sigismember(&pending, SIGSYS) != 1) {
        fail("sig_ign worker did not see pending SIGSYS");
    }
    pthread_barrier_wait(arg->b3);
    pthread_barrier_wait(arg->b4);

    /* After main set SIGSYS to SIG_IGN, this thread's pending SIGSYS must be cleared! */
    sigemptyset(&pending);
    if (sigpending(&pending) != 0 || sigismember(&pending, SIGSYS) != 0) {
        fail("SIG_IGN did not clear pending SIGSYS on another thread");
    }
    if (pthread_sigmask(SIG_UNBLOCK, &only_sigsys, NULL) != 0) {
        fail("sig_ign worker unblock SIGSYS");
    }
    return NULL;
}

static int run_blocked_sigsys_pending_test(void) {
    const char *expected = expected_exe_path();
    g_expected_exe = expected;

    struct sigaction act;
    memset(&act, 0, sizeof act);
    act.sa_sigaction = sigsys_queued_info_handler;
    act.sa_flags = SA_SIGINFO;
    sigemptyset(&act.sa_mask);
    if (sigaction(SIGSYS, &act, NULL) != 0) {
        fail("sigaction SIGSYS SA_SIGINFO");
    }
    struct sigaction sa_u1;
    memset(&sa_u1, 0, sizeof sa_u1);
    sa_u1.sa_handler = sigusr1_queued_handler;
    sigemptyset(&sa_u1.sa_mask);
    if (sigaction(SIGUSR1, &sa_u1, NULL) != 0) {
        fail("sigaction SIGUSR1 merge");
    }

    sigset_t only_sigsys, only_usr1, both, empty, pending;
    sigemptyset(&only_sigsys);
    sigaddset(&only_sigsys, SIGSYS);
    sigemptyset(&only_usr1);
    sigaddset(&only_usr1, SIGUSR1);
    sigemptyset(&both);
    sigaddset(&both, SIGSYS);
    sigaddset(&both, SIGUSR1);
    sigemptyset(&empty);

    /* 1. Block SIGSYS + SIGUSR1, sigqueue SIGSYS with payload + raise SIGUSR1,
     *    verify rt_sigpending merges both kernel SIGUSR1 and virtual SIGSYS,
     *    then unblock SIGSYS via SIG_SETMASK and verify si_value fidelity. */
    if (sigprocmask(SIG_BLOCK, &both, NULL) != 0) {
        fail("block SIGSYS+SIGUSR1");
    }
    g_queued_sigsys_count = 0;
    g_queued_sigsys_val = 0;
    g_queued_usr1_count = 0;
    g_handler_readlink_ok = 0;
    union sigval sv1;
    sv1.sival_int = 0x13572468;
    if (sigqueue(getpid(), SIGSYS, sv1) != 0) {
        fail("sigqueue blocked SIGSYS");
    }
    if (raise(SIGUSR1) != 0) {
        fail("raise blocked SIGUSR1");
    }
    if (g_queued_sigsys_count != 0 || g_queued_usr1_count != 0) {
        fail("blocked signals were delivered immediately");
    }

    sigemptyset(&pending);
    if (sigpending(&pending) != 0
        || sigismember(&pending, SIGSYS) != 1
        || sigismember(&pending, SIGUSR1) != 1) {
        fail("sigpending did not merge kernel SIGUSR1 and virtual SIGSYS");
    }

    /* Intercepted syscalls must keep working while SIGSYS is virtually blocked+pending. */
    assert_readlink_matches_expected("/proc/self/exe", expected);

    /* Unblocking SIGSYS via SIG_SETMASK delivers the deferred signal with exact si_value. */
    if (sigprocmask(SIG_SETMASK, &only_usr1, NULL) != 0) {
        fail("SIG_SETMASK unblock SIGSYS");
    }
    if (g_queued_sigsys_count != 1 || g_queued_sigsys_val != 0x13572468 || !g_handler_readlink_ok) {
        fail("pending SIGSYS was not delivered on SIG_SETMASK with matching si_value");
    }
    if (g_queued_usr1_count != 0) {
        fail("SIGUSR1 was prematurely delivered during SIG_SETMASK");
    }
    if (sigprocmask(SIG_SETMASK, &empty, NULL) != 0) {
        fail("SIG_SETMASK unblock SIGUSR1");
    }
    if (g_queued_usr1_count != 1) {
        fail("pending SIGUSR1 was not delivered on SIG_SETMASK");
    }

    /* 2. Also verify SIG_UNBLOCK delivers sigqueued SIGSYS with si_value fidelity. */
    if (sigprocmask(SIG_BLOCK, &only_sigsys, NULL) != 0) {
        fail("block SIGSYS for SIG_UNBLOCK");
    }
    g_queued_sigsys_count = 0;
    g_queued_sigsys_val = 0;
    union sigval sv2;
    sv2.sival_int = 0x24681357;
    if (sigqueue(getpid(), SIGSYS, sv2) != 0) {
        fail("sigqueue SIGSYS for SIG_UNBLOCK");
    }
    if (sigprocmask(SIG_UNBLOCK, &only_sigsys, NULL) != 0) {
        fail("unblock SIGSYS");
    }
    if (g_queued_sigsys_count != 1 || g_queued_sigsys_val != 0x24681357) {
        fail("pending SIGSYS was not delivered on SIG_UNBLOCK with matching si_value");
    }
    sigemptyset(&pending);
    if (sigpending(&pending) != 0 || sigismember(&pending, SIGSYS) != 0) {
        fail("SIGSYS still pending after delivery");
    }

    /* 3. SIG_DFL while blocked must NOT kill until unblocked; switching to SIG_IGN
     *    before unblocking discards it safely across ALL threads in the process. */
    memset(&act, 0, sizeof act);
    act.sa_handler = SIG_DFL;
    if (sigaction(SIGSYS, &act, NULL) != 0) {
        fail("sigaction SIGSYS SIG_DFL");
    }
    if (sigprocmask(SIG_BLOCK, &only_sigsys, NULL) != 0) {
        fail("re-block SIGSYS");
    }
    if (raise(SIGSYS) != 0) {
        fail("raise blocked SIGSYS with SIG_DFL");
    }
    sigemptyset(&pending);
    if (sigpending(&pending) != 0 || sigismember(&pending, SIGSYS) != 1) {
        fail("sigpending missing SIG_DFL blocked SIGSYS");
    }

    pthread_barrier_t b1, b2, b3, b4;
    pthread_barrier_init(&b1, NULL, 2);
    pthread_barrier_init(&b2, NULL, 2);
    pthread_barrier_init(&b3, NULL, 2);
    pthread_barrier_init(&b4, NULL, 2);
    struct sig_ign_worker_arg warg = {&b1, &b2, &b3, &b4};
    pthread_t worker;
    if (pthread_create(&worker, NULL, sig_ign_pending_worker, &warg) != 0) {
        fail("pthread_create sig_ign worker");
    }
    pthread_barrier_wait(&b1);
    pthread_kill(worker, SIGSYS);
    pthread_barrier_wait(&b2);
    pthread_barrier_wait(&b3);

    act.sa_handler = SIG_IGN;
    if (sigaction(SIGSYS, &act, NULL) != 0) {
        fail("sigaction SIGSYS SIG_IGN");
    }
    pthread_barrier_wait(&b4);
    pthread_join(worker, NULL);
    pthread_barrier_destroy(&b1);
    pthread_barrier_destroy(&b2);
    pthread_barrier_destroy(&b3);
    pthread_barrier_destroy(&b4);

    if (sigprocmask(SIG_UNBLOCK, &only_sigsys, NULL) != 0) {
        fail("unblock ignored SIGSYS");
    }
    assert_readlink_matches_expected("/proc/self/exe", expected);

    write_all(STDOUT_FILENO, "blocked-sigsys-pending-ok\n", 26);
    return 0;
}

/* ---- wait-mask-sigsys subcommand ---- */

static void *send_sigsys_delayed_thread(void *raw) {
    struct storm_arg *arg = (struct storm_arg *)raw;
    struct timespec ts = {0, 10 * 1000 * 1000}; /* 10 ms */
    nanosleep(&ts, NULL);
    pthread_kill(arg->target, SIGSYS);
    return NULL;
}

static int run_wait_mask_sigsys_test(void) {
    g_expected_exe = expected_exe_path();
    struct sigaction act;
    memset(&act, 0, sizeof act);
    act.sa_handler = sigsys_plain_handler;
    sigemptyset(&act.sa_mask);
    if (sigaction(SIGSYS, &act, NULL) != 0) {
        fail("sigaction SIGSYS wait-mask");
    }

    /* 1. Block SIGSYS, raise it, call ppoll with SIGSYS still in wait mask
     *    (must stay deferred), then call sigsuspend with empty wait mask
     *    (must deliver pending SIGSYS and return EINTR while keeping SIGSYS
     *    blocked afterward). */
    sigset_t only_sigsys, empty, cur;
    sigemptyset(&only_sigsys);
    sigaddset(&only_sigsys, SIGSYS);
    sigemptyset(&empty);

    if (sigprocmask(SIG_BLOCK, &only_sigsys, NULL) != 0) {
        fail("block SIGSYS wait-mask");
    }
    g_sigsys_plain_count = 0;
    if (raise(SIGSYS) != 0) {
        fail("raise SIGSYS wait-mask");
    }
    struct timespec short_tmo = {0, 5 * 1000 * 1000};
    if (ppoll(NULL, 0, &short_tmo, &only_sigsys) != 0) {
        fail("ppoll with SIGSYS blocked should time out");
    }
    if (g_sigsys_plain_count != 0) {
        fail("ppoll delivered SIGSYS despite SIGSYS in wait mask");
    }

    errno = 0;
    if (sigsuspend(&empty) != -1 || errno != EINTR) {
        fail("sigsuspend(&empty) did not return EINTR for pending SIGSYS");
    }
    if (g_sigsys_plain_count != 1) {
        fail("sigsuspend(&empty) did not deliver pending SIGSYS");
    }
    sigemptyset(&cur);
    if (sigprocmask(SIG_SETMASK, NULL, &cur) != 0 || sigismember(&cur, SIGSYS) != 1) {
        fail("sigsuspend did not preserve blocked SIGSYS mask");
    }
    if (sigprocmask(SIG_UNBLOCK, &only_sigsys, NULL) != 0) {
        fail("unblock SIGSYS after sigsuspend");
    }

    /* 2. With SIGSYS unblocked, enter ppoll(&only_sigsys) while a helper sends
     *    SIGSYS; SIGSYS must NOT interrupt ppoll and must be delivered once
     *    ppoll restores the unblocked mask. */
    g_sigsys_plain_count = 0;
    struct storm_arg sarg = {pthread_self()};
    pthread_t sender;
    if (pthread_create(&sender, NULL, send_sigsys_delayed_thread, &sarg) != 0) {
        fail("pthread_create sigsys sender");
    }
    struct timespec wait_tmo = {0, 40 * 1000 * 1000}; /* 40 ms */
    int rc = ppoll(NULL, 0, &wait_tmo, &only_sigsys);
    pthread_join(sender, NULL);
    if (rc != 0) {
        fail("ppoll(&only_sigsys) was interrupted by masked SIGSYS");
    }
    if (g_sigsys_plain_count != 1) {
        fail("SIGSYS deferred during ppoll was not delivered on return");
    }

    /* 3. Same deferred-SIGSYS check via epoll_pwait(&only_sigsys). */
    int epfd = epoll_create1(EPOLL_CLOEXEC);
    if (epfd < 0) {
        fail("epoll_create1 wait-mask");
    }
    g_sigsys_plain_count = 0;
    if (pthread_create(&sender, NULL, send_sigsys_delayed_thread, &sarg) != 0) {
        close(epfd);
        fail("pthread_create epoll sigsys sender");
    }
    struct epoll_event ev;
    rc = epoll_pwait(epfd, &ev, 1, 40, &only_sigsys);
    pthread_join(sender, NULL);
    close(epfd);
    if (rc != 0) {
        fail("epoll_pwait(&only_sigsys) was interrupted by masked SIGSYS");
    }
    if (g_sigsys_plain_count != 1) {
        fail("SIGSYS deferred during epoll_pwait was not delivered on return");
    }

    write_all(STDOUT_FILENO, "wait-mask-sigsys-ok\n", 20);
    return 0;
}

/* ---- tid-churn subcommand ---- */

static void *churn_thread_fn(void *unused) {
    (void)unused;
    /* Block SIGSYS before exiting (just like glibc's thread-exit path). */
    sigset_t set;
    sigemptyset(&set);
    sigaddset(&set, SIGSYS);
    if (pthread_sigmask(SIG_BLOCK, &set, NULL) != 0) {
        fail("churn thread block SIGSYS");
    }
    return NULL;
}

static void *post_churn_check_fn(void *unused) {
    (void)unused;
    sigset_t set, cur;
    sigemptyset(&cur);
    if (pthread_sigmask(SIG_SETMASK, NULL, &cur) != 0 || sigismember(&cur, SIGSYS) != 0) {
        fail("post-churn thread started with stale blocked SIGSYS");
    }
    sigemptyset(&set);
    sigaddset(&set, SIGSYS);
    if (pthread_sigmask(SIG_BLOCK, &set, NULL) != 0) {
        fail("post-churn block SIGSYS");
    }
    sigemptyset(&cur);
    if (pthread_sigmask(SIG_SETMASK, NULL, &cur) != 0 || sigismember(&cur, SIGSYS) != 1) {
        fail("post-churn SIGSYS not recorded blocked (reaping failed)");
    }
    if (pthread_sigmask(SIG_UNBLOCK, &set, NULL) != 0) {
        fail("post-churn unblock SIGSYS");
    }
    return NULL;
}

static int run_tid_churn_test(void) {
    /* Exceed MAX_TID_SLOTS (2048) to force dead-TID reaping. */
    pthread_attr_t attr;
    if (pthread_attr_init(&attr) != 0) {
        fail("pthread_attr_init");
    }
    (void)pthread_attr_setstacksize(&attr, 64 * 1024);
    for (int i = 0; i < 2200; i++) {
        pthread_t t;
        if (pthread_create(&t, &attr, churn_thread_fn, NULL) != 0) {
            fail("pthread_create churn");
        }
        if (pthread_join(t, NULL) != 0) {
            fail("pthread_join churn");
        }
    }
    pthread_t check;
    if (pthread_create(&check, &attr, post_churn_check_fn, NULL) != 0) {
        fail("pthread_create post-churn");
    }
    if (pthread_join(check, NULL) != 0) {
        fail("pthread_join post-churn");
    }
    pthread_attr_destroy(&attr);

    write_all(STDOUT_FILENO, "tid-churn-ok\n", 13);
    return 0;
}

/* ---- fork-sigmask-and-lock subcommand ---- */

static volatile sig_atomic_t g_hammer_stop = 0;

static void *sigaction_hammer_thread(void *unused) {
    (void)unused;
    struct sigaction act, old;
    memset(&act, 0, sizeof act);
    act.sa_handler = SIG_IGN;
    while (!g_hammer_stop) {
        (void)sigaction(SIGSYS, &act, &old);
        act.sa_handler = (act.sa_handler == SIG_IGN) ? sigsys_plain_handler : SIG_IGN;
    }
    return NULL;
}

static int run_fork_sigmask_and_lock_test(void) {
    const char *expected = expected_exe_path();
    g_expected_exe = expected;

    sigset_t set;
    sigemptyset(&set);
    sigaddset(&set, SIGSYS);
    sigaddset(&set, SIGUSR2);
    if (sigprocmask(SIG_BLOCK, &set, NULL) != 0) {
        fail("parent block SIGSYS+SIGUSR2");
    }

    g_hammer_stop = 0;
    pthread_t hammer;
    if (pthread_create(&hammer, NULL, sigaction_hammer_thread, NULL) != 0) {
        fail("pthread_create hammer");
    }

    for (int i = 0; i < 20; i++) {
        pid_t pid = fork();
        if (pid < 0) {
            fail("fork");
        }
        if (pid == 0) {
            /* Must not deadlock on ACTION_LOCK even if hammer held it during fork. */
            struct sigaction old;
            memset(&old, 0, sizeof old);
            if (sigaction(SIGSYS, NULL, &old) != 0) {
                _exit(10);
            }
            sigset_t cur;
            sigemptyset(&cur);
            if (sigprocmask(SIG_SETMASK, NULL, &cur) != 0
                || sigismember(&cur, SIGSYS) != 1
                || sigismember(&cur, SIGUSR2) != 1) {
                _exit(11);
            }
            char buf[PATH_MAX];
            ssize_t n = readlink("/proc/self/exe", buf, sizeof buf - 1);
            if (n < 0) {
                _exit(12);
            }
            buf[n] = '\0';
            if (strcmp(buf, expected) != 0) {
                _exit(13);
            }
            _exit(0);
        }
        int status = 0;
        if (waitpid(pid, &status, 0) != pid || !WIFEXITED(status) || WEXITSTATUS(status) != 0) {
            fail("fork child failed");
        }
    }

#ifdef SYS_fork
    /* Also exercise raw SYS_fork (x86-64 BPF cold branch) with SIGSYS blocked. */
    {
        pid_t pid = (pid_t)syscall(SYS_fork);
        if (pid < 0) {
            fail("raw SYS_fork");
        }
        if (pid == 0) {
            sigset_t cur;
            sigemptyset(&cur);
            if (sigprocmask(SIG_SETMASK, NULL, &cur) != 0
                || sigismember(&cur, SIGSYS) != 1
                || sigismember(&cur, SIGUSR2) != 1) {
                _exit(21);
            }
            char buf[PATH_MAX];
            ssize_t n = readlink("/proc/self/exe", buf, sizeof buf - 1);
            if (n < 0 || (buf[n] = '\0', strcmp(buf, expected) != 0)) {
                _exit(22);
            }
            _exit(0);
        }
        int status = 0;
        if (waitpid(pid, &status, 0) != pid || !WIFEXITED(status) || WEXITSTATUS(status) != 0) {
            fail("raw SYS_fork child failed");
        }
    }
#endif

    g_hammer_stop = 1;
    pthread_join(hammer, NULL);
    if (sigprocmask(SIG_UNBLOCK, &set, NULL) != 0) {
        fail("parent unblock");
    }

    write_all(STDOUT_FILENO, "fork-sigmask-and-lock-ok\n", 25);
    return 0;
}

/* ---- vfork-parent-isolation subcommand ---- */

static int run_vfork_parent_isolation_test(void) {
    g_expected_exe = expected_exe_path();
    struct sigaction act, old;
    memset(&act, 0, sizeof act);
    act.sa_handler = sigsys_plain_handler;
    sigemptyset(&act.sa_mask);
    if (sigaction(SIGSYS, &act, NULL) != 0) {
        fail("parent sigaction SIGSYS");
    }

    posix_spawnattr_t attr;
    if (posix_spawnattr_init(&attr) != 0) {
        fail("posix_spawnattr_init");
    }
    sigset_t full;
    sigfillset(&full);
    if (posix_spawnattr_setflags(&attr, POSIX_SPAWN_SETSIGDEF | POSIX_SPAWN_SETSIGMASK) != 0
        || posix_spawnattr_setsigdefault(&attr, &full) != 0
        || posix_spawnattr_setsigmask(&attr, &full) != 0) {
        fail("posix_spawnattr set flags/masks");
    }

    const char *launcher = getenv("LAUNCHER_PATH");
    char *direct_argv[] = {"true", NULL};
    char *loaded_argv[] = {(char *)launcher, "/bin/true", "true", NULL};
    const char *spawn_prog = (launcher != NULL && launcher[0] != '\0') ? launcher : "/bin/true";
    char **spawn_argv = (launcher != NULL && launcher[0] != '\0') ? loaded_argv : direct_argv;

    pid_t pid = -1;
    int rc = posix_spawn(&pid, spawn_prog, NULL, &attr, spawn_argv, environ);
    posix_spawnattr_destroy(&attr);
    if (rc != 0) {
        errno = rc;
        fail("posix_spawn /bin/true");
    }
    int status = 0;
    if (waitpid(pid, &status, 0) != pid || !WIFEXITED(status) || WEXITSTATUS(status) != 0) {
        fail("waitpid /bin/true");
    }

    /* Parent's virtual SIGSYS handler and sigmask must remain intact! */
    memset(&old, 0, sizeof old);
    if (sigaction(SIGSYS, NULL, &old) != 0 || old.sa_handler != sigsys_plain_handler) {
        fail("CLONE_VM child clobbered parent virtual SIGSYS handler");
    }
    sigset_t cur;
    sigemptyset(&cur);
    if (sigprocmask(SIG_SETMASK, NULL, &cur) != 0 || sigismember(&cur, SIGSYS) != 0) {
        fail("CLONE_VM child clobbered parent virtual SIGSYS mask");
    }
    g_sigsys_plain_count = 0;
    if (raise(SIGSYS) != 0 || g_sigsys_plain_count != 1) {
        fail("parent custom SIGSYS handler failed after posix_spawn");
    }

    write_all(STDOUT_FILENO, "vfork-parent-isolation-ok\n", 26);
    return 0;
}

/* ---- tiny-vfork-stack subcommand (5 KiB stack above PROT_NONE guard) ---- */

static int tiny_vfork_child_fn(void *arg) {
    const char *expected = (const char *)arg;
    uint64_t mask = 0;
    if (syscall(SYS_rt_sigprocmask, SIG_SETMASK, NULL, &mask, sizeof mask) != 0) {
        _exit(20);
    }
    long fd = syscall(SYS_openat, AT_FDCWD, "/dev/null", O_RDONLY | O_CLOEXEC, 0);
    if (fd < 0) {
        _exit(21);
    }
    syscall(SYS_close, fd);

    char buf[256];
    long n = syscall(SYS_readlinkat, AT_FDCWD, "/proc/self/exe", buf, sizeof buf - 1);
    if (n <= 0) {
        _exit(22);
    }
    buf[n] = '\0';
    if (strcmp(buf, expected) != 0) {
        _exit(23);
    }
    _exit(42);
}

static int run_tiny_vfork_stack_test(void) {
    const char *expected = expected_exe_path();
    long page = sysconf(_SC_PAGESIZE);
    if (page <= 0) {
        page = 4096;
    }
    /* Allocate guard page + 2 usable pages, but place child SP only 5120 bytes
     * above the PROT_NONE guard page (matching AArch64 musl posix_spawn). */
    size_t total = (size_t)page * 3;
    unsigned char *mem = mmap(NULL, total, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (mem == MAP_FAILED) {
        fail("mmap tiny stack");
    }
    if (mprotect(mem, (size_t)page, PROT_NONE) != 0) {
        fail("mprotect guard page");
    }
    unsigned char *stack_top = mem + (size_t)page + 5120;
    /* Align down to 16 bytes for ABI compliance. */
    stack_top = (unsigned char *)((uintptr_t)stack_top & ~(uintptr_t)15);

    pid_t pid = clone(tiny_vfork_child_fn, stack_top, CLONE_VM | CLONE_VFORK | SIGCHLD, (void *)expected);
    if (pid < 0) {
        fail("clone tiny vfork stack");
    }
    int status = 0;
    if (waitpid(pid, &status, 0) != pid || !WIFEXITED(status) || WEXITSTATUS(status) != 42) {
        fail("tiny vfork stack child failed or overflowed guard page");
    }
    munmap(mem, total);

    write_all(STDOUT_FILENO, "tiny-vfork-stack-ok\n", 20);
    return 0;
}

/* ---- abseil-address-probe subcommand (Abseil AddressIsReadable) ---- */

static int run_abseil_address_probe_test(void) {
    /* Abseil AddressIsReadable: how = ~0, set = probe_addr. The kernel copies
     * set first (EFAULT) and checks how second (EINVAL), and the loader must
     * pass the probe to the kernel without reading the address itself. */
    const uintptr_t bad_low = 1;
    const uintptr_t bad_high = 0xdeadbeef0000ULL;
    errno = 0;
    if (syscall(SYS_rt_sigprocmask, ~0L, bad_low, 0, 8) != -1 || errno != EFAULT) {
        fail("Abseil rt_sigprocmask(~0, bad_low) did not return EFAULT");
    }
    errno = 0;
    if (syscall(SYS_rt_sigprocmask, ~0L, bad_high, 0, 8) != -1 || errno != EFAULT) {
        fail("Abseil rt_sigprocmask(~0, bad_high) did not return EFAULT");
    }
    uint64_t valid_set = 0;
    errno = 0;
    if (syscall(SYS_rt_sigprocmask, ~0L, &valid_set, 0, 8) != -1 || errno != EINVAL) {
        fail("Abseil rt_sigprocmask(~0, valid) did not return EINVAL");
    }

    write_all(STDOUT_FILENO, "abseil-address-probe-ok\n", 24);
    return 0;
}

/* ---- vfork-shared-stack-deep subcommand (Opus M1 regression test) ---- */

__attribute__((noinline)) static void touch_deep_vfork_stack(const char *expected) {
    volatile unsigned char buf[16384];
    for (size_t i = 0; i < sizeof buf; i += 64) {
        buf[i] = (unsigned char)(i ^ 0xa5);
    }
    int fd = openat(AT_FDCWD, "/dev/null", O_RDONLY | O_CLOEXEC);
    if (fd >= 0) {
        close(fd);
    }
    char linkbuf[PATH_MAX];
    ssize_t n = readlink("/proc/self/exe", linkbuf, sizeof linkbuf - 1);
    if (n <= 0 || (size_t)n != strlen(expected) || memcmp(linkbuf, expected, (size_t)n) != 0) {
        _exit(44);
    }
    if (buf[0] != 0xa5 || buf[16320] != (unsigned char)(16320 ^ 0xa5)) {
        _exit(45);
    }
}

__attribute__((noinline)) static long raw_shared_stack_vfork_and_check(const char *expected) {
    /* Keep known callee-saved FP/SIMD sentinels live across raw
     * `clone(CLONE_VM | CLONE_VFORK | SIGCHLD, NULL, ...)`. On AArch64, if our
     * SIGSYS frame sat below the parent's SP, the child's 16 KiB frame would
     * overwrite saved `d8..d15` / GPRs in the parent's `ucontext`. */
    double s0 = 1.25;
    double s1 = 2.5;
    double s2 = 3.75;
    double s3 = 4.125;
    double s4 = 5.625;
    double s5 = 6.875;
    double s6 = 7.375;
    double s7 = 8.9375;
#if defined(__aarch64__)
    __asm__ volatile(""
                     : "+w"(s0), "+w"(s1), "+w"(s2), "+w"(s3), "+w"(s4), "+w"(s5), "+w"(s6),
                       "+w"(s7));
#elif defined(__x86_64__)
    __asm__ volatile(""
                     : "+x"(s0), "+x"(s1), "+x"(s2), "+x"(s3), "+x"(s4), "+x"(s5), "+x"(s6),
                       "+x"(s7));
#endif

    /* `vfork()` uses `clone(CLONE_VM | CLONE_VFORK | SIGCHLD, NULL, ...)` on
     * AArch64 (and `vfork` on x86-64) while preserving the return address off
     * the shared stack. */
    pid_t pid = vfork();
    if (pid == 0) {
        touch_deep_vfork_stack(expected);
        _exit(0);
    }
    if (pid < 0) {
        return -1;
    }

    int status = 0;
    if (waitpid((pid_t)pid, &status, 0) != (pid_t)pid || !WIFEXITED(status) ||
        WEXITSTATUS(status) != 0) {
        fail("vfork-shared-stack-deep child failed");
    }

    if (s0 != 1.25 || s1 != 2.5 || s2 != 3.75 || s3 != 4.125 || s4 != 5.625 || s5 != 6.875 ||
        s6 != 7.375 || s7 != 8.9375) {
        fail("parent FP/SIMD callee-saved registers were corrupted by shared-stack vfork child");
    }
    return 0;
}

static int run_vfork_shared_stack_deep_test(void) {
    const char *expected = expected_exe_path();
    for (int i = 0; i < 8; i++) {
        if (raw_shared_stack_vfork_and_check(expected) != 0) {
            fail("raw_shared_stack_vfork_and_check failed");
        }
    }
    write_all(STDOUT_FILENO, "vfork-shared-stack-deep-ok\n", 27);
    return 0;
}

/* ---- clone-custom-stack-new-mm subcommand (Grok M2 regression test) ---- */

static int clone_custom_stack_child_fn(void *arg) {
    const char *expected = (const char *)arg;
    struct sigaction act, old;
    memset(&act, 0, sizeof act);
    memset(&old, 0, sizeof old);
    act.sa_handler = sigsys_plain_handler;
    sigemptyset(&act.sa_mask);
    if (sigaction(SIGSYS, &act, NULL) != 0) {
        _exit(50);
    }
    if (sigaction(SIGSYS, NULL, &old) != 0 || old.sa_handler != sigsys_plain_handler) {
        _exit(51);
    }
    sigset_t s;
    sigemptyset(&s);
    sigaddset(&s, SIGSYS);
    if (sigprocmask(SIG_BLOCK, &s, NULL) != 0 || sigprocmask(SIG_UNBLOCK, &s, NULL) != 0) {
        _exit(52);
    }
    char linkbuf[PATH_MAX];
    ssize_t n = readlink("/proc/self/exe", linkbuf, sizeof linkbuf - 1);
    if (n <= 0 || (size_t)n != strlen(expected) || memcmp(linkbuf, expected, (size_t)n) != 0) {
        _exit(53);
    }
    return 0;
}

static int run_clone_custom_stack_new_mm_test(void) {
    const char *expected = expected_exe_path();
    size_t stack_sz = 64 * 1024;
    unsigned char *stack_mem =
        mmap(NULL, stack_sz, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (stack_mem == MAP_FAILED) {
        fail("mmap custom clone stack");
    }
    void *child_sp = (void *)(((uintptr_t)(stack_mem + stack_sz)) & ~(uintptr_t)15);

    /* `clone(fn, child_sp, SIGCHLD, ...)` creates a new address space
     * (`CLONE_VM == 0`) with a non-NULL child stack. `on_fork_child` MUST still
     * run so `OWNER_PID` updates and child `sigaction(SIGSYS)` /
     * `rt_sigprocmask` work normally. */
    int pid = clone(clone_custom_stack_child_fn, child_sp, SIGCHLD, (void *)expected);
    if (pid < 0) {
        fail("clone(SIGCHLD, child_sp) failed");
    }

    int status = 0;
    if (waitpid((pid_t)pid, &status, 0) != (pid_t)pid || !WIFEXITED(status) ||
        WEXITSTATUS(status) != 0) {
        char msg[128];
        snprintf(msg, sizeof msg, "clone-custom-stack-new-mm child failed with status=%d", status);
        fail(msg);
    }
    munmap(stack_mem, stack_sz);
    write_all(STDOUT_FILENO, "clone-custom-stack-new-mm-ok\n", 29);
    return 0;
}

/* ---- pselect-timeout-deferred-sigsys subcommand (Grok M1 regression test) ---- */

struct pselect_sender_arg {
    pid_t pid;
    pid_t tid;
};

static void *pselect_sigsys_sender(void *raw) {
    struct pselect_sender_arg *arg = (struct pselect_sender_arg *)raw;
    struct timespec delay = {0, 40 * 1000 * 1000}; /* 40 ms */
    nanosleep(&delay, NULL);
    syscall(SYS_tgkill, arg->pid, arg->tid, SIGSYS);
    return NULL;
}

static long long monotonic_ms(void) {
    struct timespec ts;
    if (clock_gettime(CLOCK_MONOTONIC, &ts) != 0) {
        fail("clock_gettime");
    }
    return (long long)ts.tv_sec * 1000LL + (long long)(ts.tv_nsec / 1000000L);
}

static int run_pselect_timeout_deferred_sigsys_test(void) {
    struct sigaction sa;
    memset(&sa, 0, sizeof sa);
    sa.sa_handler = sigsys_plain_handler;
    sigemptyset(&sa.sa_mask);
    if (sigaction(SIGSYS, &sa, NULL) != 0) {
        fail("sigaction SIGSYS pselect");
    }
    g_sigsys_plain_count = 0;
    g_expected_exe = expected_exe_path();

    sigset_t mask;
    sigemptyset(&mask);
    sigaddset(&mask, SIGSYS);

    struct pselect_sender_arg sarg = {getpid(), (pid_t)syscall(SYS_gettid)};
    pthread_t sender;
    if (pthread_create(&sender, NULL, pselect_sigsys_sender, &sarg) != 0) {
        fail("pthread_create pselect sender");
    }

    struct timespec ts = {0, 180 * 1000 * 1000}; /* 180 ms */
    long long t0 = monotonic_ms();
    int rc = pselect(0, NULL, NULL, NULL, &ts, &mask);
    long long elapsed = monotonic_ms() - t0;
    pthread_join(sender, NULL);

    if (rc != 0) {
        char msg[128];
        snprintf(msg, sizeof msg, "pselect expected 0 on timeout, got rc=%d errno=%d", rc, errno);
        fail(msg);
    }
    if (g_sigsys_plain_count != 1) {
        fail("pselect did not deliver deferred SIGSYS upon exit");
    }
    /* If pselect restarted its full 180ms timeout after the 40ms SIGSYS, it
     * would take ~220ms+; fire a second check with 3 spaced SIGSYS signals?
     * Check that elapsed stayed well below 2x timeout (320ms). */
    if (elapsed < 150 || elapsed > 320) {
        char msg[128];
        snprintf(msg, sizeof msg, "pselect elapsed %lld ms outside [150, 320]", elapsed);
        fail(msg);
    }

    write_all(STDOUT_FILENO, "pselect-timeout-deferred-sigsys-ok\n", 35);
    return 0;
}

/* ---- proc-sibling-tid-exe subcommand (Grok M3 regression test) ---- */

struct sibling_tid_state {
    volatile pid_t tid;
    volatile int stop;
};

static void *sibling_tid_worker(void *raw) {
    struct sibling_tid_state *st = (struct sibling_tid_state *)raw;
    st->tid = (pid_t)syscall(SYS_gettid);
    while (!st->stop) {
        struct timespec ts = {0, 2 * 1000 * 1000};
        nanosleep(&ts, NULL);
    }
    return NULL;
}

static int run_proc_sibling_tid_exe_test(void) {
    const char *expected = expected_exe_path();
    struct sibling_tid_state st = {0, 0};
    pthread_t th;
    if (pthread_create(&th, NULL, sibling_tid_worker, &st) != 0) {
        fail("pthread_create sibling_tid_worker");
    }
    while (st.tid == 0) {
        struct timespec ts = {0, 1000 * 1000};
        nanosleep(&ts, NULL);
    }

    char path[64];
    snprintf(path, sizeof path, "/proc/%d/exe", (int)st.tid);
    assert_readlink_matches_expected(path, expected);

    struct stat expected_st, sibling_st;
    if (stat(expected, &expected_st) != 0 || stat(path, &sibling_st) != 0) {
        fail("stat /proc/<sibling_tid>/exe");
    }
    if (expected_st.st_dev != sibling_st.st_dev || expected_st.st_ino != sibling_st.st_ino) {
        fail("stat /proc/<sibling_tid>/exe did not match target executable");
    }

    st.stop = 1;
    pthread_join(th, NULL);
    write_all(STDOUT_FILENO, "proc-sibling-tid-exe-ok\n", 24);
    return 0;
}

/* ---- pselect-simultaneous-eintr subcommand (Grok M2 + Opus L1 regression) ---- */

static volatile sig_atomic_t g_eintr_usr1_hits = 0;
static volatile pid_t g_eintr_pid = 0;
static volatile pid_t g_eintr_tid = 0;

static void pselect_eintr_usr1_handler(int sig) {
    if (sig == SIGUSR1) {
        g_eintr_usr1_hits++;
    }
}

static void *pselect_usr1_sender(void *raw) {
    struct pselect_sender_arg *arg = (struct pselect_sender_arg *)raw;
    struct timespec delay = {0, 30 * 1000 * 1000}; /* 30 ms */
    nanosleep(&delay, NULL);
    syscall(SYS_tgkill, arg->pid, arg->tid, SIGUSR1);
    return NULL;
}

static atomic_int g_pselect_fdset_done = 0;

static void *pselect_late_sigsys_sender(void *raw) {
    struct pselect_sender_arg *arg = (struct pselect_sender_arg *)raw;
    /* Fire SIGSYS every 3 ms until pselect returns so at least one deferred
     * SIGSYS lands at or after the 20 ms timeout expiry (exercising the
     * zero-timeout replay path that clears fd_sets). */
    for (int i = 0; i < 40 && !atomic_load(&g_pselect_fdset_done); i++) {
        struct timespec delay = {0, 3 * 1000 * 1000}; /* 3 ms */
        nanosleep(&delay, NULL);
        if (!atomic_load(&g_pselect_fdset_done)) {
            syscall(SYS_tgkill, arg->pid, arg->tid, SIGSYS);
        }
    }
    return NULL;
}

static int run_pselect_simultaneous_eintr_test(void) {
    struct sigaction sa_sys;
    memset(&sa_sys, 0, sizeof sa_sys);
    sa_sys.sa_handler = sigsys_plain_handler;
    sigemptyset(&sa_sys.sa_mask);
    if (sigaction(SIGSYS, &sa_sys, NULL) != 0) {
        fail("sigaction SIGSYS simultaneous-eintr");
    }

    struct sigaction sa_u1;
    memset(&sa_u1, 0, sizeof sa_u1);
    sa_u1.sa_handler = pselect_eintr_usr1_handler;
    sigemptyset(&sa_u1.sa_mask);
    if (sigaction(SIGUSR1, &sa_u1, NULL) != 0) {
        fail("sigaction SIGUSR1 simultaneous-eintr");
    }

    g_sigsys_plain_count = 0;
    g_eintr_usr1_hits = 0;
    g_expected_exe = expected_exe_path();
    g_eintr_pid = getpid();
    g_eintr_tid = (pid_t)syscall(SYS_gettid);

    sigset_t mask;
    sigemptyset(&mask);
    sigaddset(&mask, SIGSYS);

    /* Part 1: Real SIGUSR1 + deferred SIGSYS in the same pselect wake MUST
     * still return -1/EINTR instead of swallowing EINTR and sleeping to timeout. */
    struct pselect_sender_arg sarg = {g_eintr_pid, g_eintr_tid};
    pthread_t sender;
    if (pthread_create(&sender, NULL, pselect_usr1_sender, &sarg) != 0) {
        fail("pthread_create pselect usr1 sender");
    }

    struct timespec ts = {2, 0}; /* 2 s — would clearly fail if EINTR were swallowed */
    errno = 0;
    int rc = pselect(0, NULL, NULL, NULL, &ts, &mask);
    int saved_errno = errno;
    pthread_join(sender, NULL);

    if (rc != -1 || saved_errno != EINTR) {
        char msg[128];
        snprintf(
            msg,
            sizeof msg,
            "pselect simultaneous SIGUSR1+SIGSYS expected -1/EINTR, got rc=%d errno=%d",
            rc,
            saved_errno
        );
        fail(msg);
    }
    if (g_eintr_usr1_hits != 1) {
        fail("pselect simultaneous-eintr did not observe SIGUSR1");
    }

    /* Part 2: When pselect times out after a deferred SIGSYS retry, the kernel
     * MUST clear non-ready fds in readfds (never returning 0 with stale bits). */
    int pfd[2];
    if (pipe(pfd) != 0) {
        fail("pipe for pselect fdset clear");
    }
    fd_set rfds;
    FD_ZERO(&rfds);
    FD_SET(pfd[0], &rfds);
    g_sigsys_plain_count = 0;
    atomic_store(&g_pselect_fdset_done, 0);
    if (pthread_create(&sender, NULL, pselect_late_sigsys_sender, &sarg) != 0) {
        fail("pthread_create pselect late sigsys sender");
    }
    struct timespec short_ts = {0, 20 * 1000 * 1000}; /* 20 ms */
    rc = pselect(pfd[0] + 1, &rfds, NULL, NULL, &short_ts, &mask);
    atomic_store(&g_pselect_fdset_done, 1);
    int still_set = FD_ISSET(pfd[0], &rfds);
    pthread_join(sender, NULL);
    close(pfd[0]);
    close(pfd[1]);
    if (rc != 0 || still_set != 0 || g_sigsys_plain_count < 1) {
        char msg[128];
        snprintf(
            msg,
            sizeof msg,
            "pselect timeout after deferred SIGSYS failed: rc=%d fd_set=%d sigsys=%d",
            rc,
            still_set,
            (int)g_sigsys_plain_count
        );
        fail(msg);
    }

    /* Part 3: Raw `SYS_ppoll` with a deferred SIGSYS must write back remaining
     * time (0 on expiry) into the caller's timespec. */
    struct timespec ppoll_ts = {0, 20 * 1000 * 1000}; /* 20 ms */
    uint64_t k_mask = UINT64_C(1) << (SIGSYS - 1);
    g_sigsys_plain_count = 0;
    atomic_store(&g_pselect_fdset_done, 0);
    if (pthread_create(&sender, NULL, pselect_late_sigsys_sender, &sarg) != 0) {
        fail("pthread_create ppoll sigsys sender");
    }
    long ppoll_rc = syscall(SYS_ppoll, NULL, 0UL, &ppoll_ts, &k_mask, 8UL);
    atomic_store(&g_pselect_fdset_done, 1);
    pthread_join(sender, NULL);
    if (ppoll_rc != 0 || ppoll_ts.tv_sec != 0 || ppoll_ts.tv_nsec != 0) {
        char msg[128];
        snprintf(
            msg,
            sizeof msg,
            "raw ppoll timeout write-back failed: rc=%ld ts=%ld.%09ld",
            ppoll_rc,
            (long)ppoll_ts.tv_sec,
            ppoll_ts.tv_nsec
        );
        fail(msg);
    }

    write_all(STDOUT_FILENO, "pselect-simultaneous-eintr-ok\n", 30);
    return 0;
}

/* ---- posix-spawnp-long-path subcommand (Opus M1 x86-64/AArch64 musl 5KiB stack) ---- */

static int run_posix_spawnp_long_path_test(void) {
    /* Build a ~3.5 KiB PATH so musl's `__execvpe` allocates a large VLA on
     * musl's 5 KiB `posix_spawn` child stack before calling `execve`. */
    const char *launcher = getenv("LAUNCHER_PATH");
    char launcher_dir[PATH_MAX];
    const char *launcher_base = NULL;
    if (launcher != NULL && launcher[0] != '\0') {
        const char *slash = strrchr(launcher, '/');
        if (slash == NULL || (size_t)(slash - launcher) >= sizeof launcher_dir) {
            fail("invalid LAUNCHER_PATH");
        }
        size_t dir_len = (size_t)(slash - launcher);
        memcpy(launcher_dir, launcher, dir_len);
        launcher_dir[dir_len] = '\0';
        launcher_base = slash + 1;
    }

    char long_path[4096];
    size_t pos = 0;
    while (pos + 40 < 3400) {
        int n = snprintf(long_path + pos, sizeof long_path - pos, "/nonexistent-fspy-dir-%04zu:", pos);
        if (n <= 0) {
            fail("snprintf long_path");
        }
        pos += (size_t)n;
    }
    if (launcher_base != NULL) {
        snprintf(long_path + pos, sizeof long_path - pos, "%s:/bin:/usr/bin", launcher_dir);
    } else {
        snprintf(long_path + pos, sizeof long_path - pos, "/bin:/usr/bin");
    }
    if (setenv("PATH", long_path, 1) != 0) {
        fail("setenv PATH");
    }

    /* 1. Missing binary walks every entry of the ~3.5 KiB PATH on the child's
     * stack and propagates ENOENT cleanly without overflowing the 5 KiB stack. */
    char *argv_missing[] = {(char *)"nonexistent_fspy_long_path_probe", NULL};
    for (int i = 0; i < 4; i++) {
        pid_t pid = -1;
        int rc = posix_spawnp(&pid, "nonexistent_fspy_long_path_probe", NULL, NULL, argv_missing, environ);
        if (rc != ENOENT) {
            char msg[128];
            snprintf(msg, sizeof msg, "posix_spawnp missing expected ENOENT, got %d", rc);
            fail(msg);
        }
    }

    /* 2. Successful `posix_spawnp` resolving the last entry of the ~3.5 KiB PATH. */
    for (int i = 0; i < 4; i++) {
        pid_t pid = -1;
        int rc;
        if (launcher_base != NULL) {
            char *argv_child[] = {(char *)launcher_base, (char *)"/bin/true", (char *)"true", NULL};
            rc = posix_spawnp(&pid, launcher_base, NULL, NULL, argv_child, environ);
        } else {
            char *argv_child[] = {(char *)"true", NULL};
            rc = posix_spawnp(&pid, "true", NULL, NULL, argv_child, environ);
        }
        if (rc != 0) {
            errno = rc;
            fail("posix_spawnp with 3.5KiB PATH");
        }
        int status = 0;
        if (waitpid(pid, &status, 0) != pid || !WIFEXITED(status) || WEXITSTATUS(status) != 0) {
            fail("posix_spawnp long-path child failed");
        }
    }

    write_all(STDOUT_FILENO, "posix-spawnp-long-path-ok\n", 26);
    return 0;
}

/* ---- altstack-8kib-storm subcommand (Opus M4 regression test) ---- */

static volatile sig_atomic_t g_small_alt_hits = 0;

static void small_alt_sigusr1_handler(int sig) {
    if (sig == SIGUSR1) {
        /* Perform a trapped syscall inside the SA_ONSTACK handler on a small
         * realistic altstack (8 KiB on x86-64 / MINSIGSTKSZ on AArch64). */
        int fd = openat(AT_FDCWD, "/dev/null", O_RDONLY | O_CLOEXEC);
        if (fd >= 0) {
            close(fd);
        }
        g_small_alt_hits++;
    }
}

static int run_altstack_8kib_storm_test(void) {
    const char *expected = expected_exe_path();
    /* Use the platform's real `SIGSTKSZ` (8 KiB on baseline x86-64, 16 KiB on
     * AArch64 where a single kernel `rt_sigframe` is ~4.7 KiB and 3 nested
     * frames need >= 3 * AT_MINSIGSTKSZ; musl's `sysconf(_SC_SIGSTKSZ)` does
     * not scale with `AT_MINSIGSTKSZ`, so query auxv directly too). */
    long sc_sigstksz = sysconf(_SC_SIGSTKSZ);
    size_t alt_sz = sc_sigstksz > 8192 ? (size_t)sc_sigstksz : 8192;
    unsigned long min_sigstksz = getauxval(51 /* AT_MINSIGSTKSZ */);
    size_t min_for_3_frames = (size_t)min_sigstksz * 3 + 1024;
    if (min_for_3_frames > alt_sz) {
        alt_sz = (min_for_3_frames + 4095) & ~(size_t)4095;
    }
    size_t page = (size_t)sysconf(_SC_PAGESIZE);
    size_t map_sz = ((alt_sz + page - 1) & ~(page - 1)) + page;
    unsigned char *mem =
        mmap(NULL, map_sz, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (mem == MAP_FAILED) {
        fail("mmap 8KiB altstack");
    }
    /* Guard page at bottom of altstack to catch any overflow immediately. */
    if (mprotect(mem, page, PROT_NONE) != 0) {
        fail("mprotect altstack guard");
    }

    stack_t ss;
    ss.ss_sp = mem + page;
    ss.ss_size = alt_sz;
    ss.ss_flags = 0;
    if (sigaltstack(&ss, NULL) != 0) {
        fail("sigaltstack 8KiB");
    }

    struct sigaction sa;
    memset(&sa, 0, sizeof sa);
    sa.sa_handler = small_alt_sigusr1_handler;
    sa.sa_flags = SA_ONSTACK | SA_RESTART;
    sigemptyset(&sa.sa_mask);
    if (sigaction(SIGUSR1, &sa, NULL) != 0) {
        fail("sigaction SIGUSR1 8KiB");
    }

    g_storm_stop = 0;
    g_small_alt_hits = 0;
    struct storm_arg sarg = {pthread_self()};
    pthread_t sender;
    if (pthread_create(&sender, NULL, storm_sender_thread, &sarg) != 0) {
        fail("pthread_create 8KiB storm sender");
    }

    sigset_t empty;
    sigemptyset(&empty);
    for (int i = 0; i < 1500; i++) {
        int fd = openat(AT_FDCWD, "/dev/null", O_RDONLY | O_CLOEXEC);
        if (fd >= 0) {
            close(fd);
        }
        assert_readlink_matches_expected("/proc/self/exe", expected);
        struct timespec zero = {0, 0};
        if (ppoll(NULL, 0, &zero, &empty) < 0 && errno != EINTR) {
            fail("8KiB storm ppoll");
        }
    }

    g_storm_stop = 1;
    pthread_join(sender, NULL);
    if (g_small_alt_hits == 0) {
        fail("8KiB storm received zero SIGUSR1 signals");
    }

    /* Disable altstack before unmapping it. */
    ss.ss_flags = SS_DISABLE;
    sigaltstack(&ss, NULL);
    munmap(mem, map_sz);
    write_all(STDOUT_FILENO, "altstack-8kib-storm-ok\n", 23);
    return 0;
}

/* ---- io-uring-enosys subcommand ---- */

static int run_io_uring_enosys_test(void) {
#ifdef SYS_io_uring_setup
    unsigned char params[120];
    memset(params, 0, sizeof params);
    errno = 0;
    long rc = syscall(SYS_io_uring_setup, 1U, params);
    if (rc != -1 || errno != ENOSYS) {
        char msg[128];
        snprintf(msg, sizeof msg, "io_uring_setup expected -1/ENOSYS, got rc=%ld errno=%d", rc, errno);
        fail(msg);
    }
#endif
    write_all(STDOUT_FILENO, "io-uring-enosys-ok\n", 19);
    return 0;
}

/* ---- inherited-sigsys subcommand ---- */

static int run_inherited_sigsys_test(void) {
    const char *expected = expected_exe_path();
    struct sigaction old;
    memset(&old, 0, sizeof old);
    if (sigaction(SIGSYS, NULL, &old) != 0 || old.sa_handler != SIG_IGN) {
        fail("inherited SIGSYS disposition was not SIG_IGN");
    }
    sigset_t cur;
    sigemptyset(&cur);
    if (sigprocmask(SIG_SETMASK, NULL, &cur) != 0
        || sigismember(&cur, SIGSYS) != 1
        || sigismember(&cur, SIGUSR1) != 1) {
        fail("inherited SIGSYS + SIGUSR1 mask was not preserved across exec");
    }
    /* Trapped syscalls must keep working while SIGSYS is virtually SIG_IGN + blocked. */
    assert_readlink_matches_expected("/proc/self/exe", expected);
    int fd = openat(AT_FDCWD, "/dev/null", O_RDONLY | O_CLOEXEC);
    if (fd < 0) {
        fail("openat /dev/null under inherited SIGSYS state");
    }
    close(fd);
    if (raise(SIGSYS) != 0) {
        fail("raise SIGSYS under inherited SIG_IGN");
    }
    write_all(STDOUT_FILENO, "inherited-sigsys-ok\n", 20);
    return 0;
}

/* ---- aarch64-x28 subcommand (Go g-register preservation on AArch64) ---- */

#ifdef __aarch64__
#define X28_SENTINEL UINT64_C(0x1122334455667788)

static volatile uint64_t g_x28_in_sigurg = 0;
static volatile uint64_t g_x28_in_sigsys = 0;

static void x28_sigurg_handler(int sig, siginfo_t *info, void *raw_uc) {
    (void)sig;
    (void)info;
    ucontext_t *uc = (ucontext_t *)raw_uc;
    g_x28_in_sigurg = uc->uc_mcontext.regs[28];
}

static void x28_sigsys_handler(int sig, siginfo_t *info, void *raw_uc) {
    (void)sig;
    (void)info;
    (void)raw_uc;
    uint64_t live_x28 = 0;
    __asm__ volatile("mov %0, x28" : "=r"(live_x28));
    g_x28_in_sigsys = live_x28;
}

static volatile sig_atomic_t g_x28_urg_done = 0;

static void *x28_sigurg_sender(void *raw) {
    struct storm_arg *arg = (struct storm_arg *)raw;
    for (int i = 0; i < 200 && !g_x28_urg_done; i++) {
        struct timespec ts = {0, 5 * 1000 * 1000}; /* 5 ms */
        nanosleep(&ts, NULL);
        if (!g_x28_urg_done) {
            pthread_kill(arg->target, SIGURG);
        }
    }
    return NULL;
}

__attribute__((noinline))
static long aarch64_syscall_with_x28(
    long nr, long a0, long a1, long a2, long a3, long a4, uint64_t *after_x28
) {
    register long x8 __asm__("x8") = nr;
    register long x0 __asm__("x0") = a0;
    register long x1 __asm__("x1") = a1;
    register long x2 __asm__("x2") = a2;
    register long x3 __asm__("x3") = a3;
    register long x4 __asm__("x4") = a4;
    uint64_t sentinel = X28_SENTINEL;
    uint64_t observed = 0;
    __asm__ volatile(
        "str x28, [sp, #-16]!\n\t"
        "mov x28, %[sent]\n\t"
        "svc #0\n\t"
        "mov %[obs], x28\n\t"
        "ldr x28, [sp], #16\n\t"
        : "+r"(x0), [obs] "=&r"(observed)
        : "r"(x8), "r"(x1), "r"(x2), "r"(x3), "r"(x4), [sent] "r"(sentinel)
        : "memory"
    );
    *after_x28 = observed;
    return x0;
}
#endif

static int run_aarch64_x28_test(void) {
#ifdef __aarch64__
    const char *fifo_path = getenv("FIFO_PATH");
    if (fifo_path == NULL || fifo_path[0] == '\0') {
        fail("missing FIFO_PATH for aarch64-x28");
    }
    if (mkfifo(fifo_path, 0600) != 0) {
        fail("mkfifo aarch64-x28");
    }

    struct sigaction sa_urg;
    memset(&sa_urg, 0, sizeof sa_urg);
    sa_urg.sa_sigaction = x28_sigurg_handler;
    sa_urg.sa_flags = SA_SIGINFO;
    sigemptyset(&sa_urg.sa_mask);
    if (sigaction(SIGURG, &sa_urg, NULL) != 0) {
        unlink(fifo_path);
        fail("sigaction SIGURG x28");
    }

    struct sigaction sa_sys;
    memset(&sa_sys, 0, sizeof sa_sys);
    sa_sys.sa_sigaction = x28_sigsys_handler;
    sa_sys.sa_flags = SA_SIGINFO;
    sigemptyset(&sa_sys.sa_mask);
    if (sigaction(SIGSYS, &sa_sys, NULL) != 0) {
        unlink(fifo_path);
        fail("sigaction SIGSYS x28");
    }

    struct storm_arg sarg = {pthread_self()};
    pthread_t sender;
    uint64_t after_x28 = 0;

    /* 1. SIGURG during masked wait (ppoll with non-null sigmask). */
    g_x28_in_sigurg = 0;
    g_x28_urg_done = 0;
    if (pthread_create(&sender, NULL, x28_sigurg_sender, &sarg) != 0) {
        unlink(fifo_path);
        fail("pthread_create x28 ppoll sender");
    }
    struct timespec tmo = {1, 0};
    uint64_t empty_mask = 0;
    long rc = aarch64_syscall_with_x28(
        SYS_ppoll, 0, 0, (long)&tmo, (long)&empty_mask, 8, &after_x28
    );
    g_x28_urg_done = 1;
    pthread_join(sender, NULL);
    if (rc != -EINTR || g_x28_in_sigurg != X28_SENTINEL || after_x28 != X28_SENTINEL) {
        unlink(fifo_path);
        fail("x28 corrupted during masked wait SIGURG");
    }

    /* 2. SIGURG during unmasked blocking openat(fifo). */
    g_x28_in_sigurg = 0;
    g_x28_urg_done = 0;
    if (pthread_create(&sender, NULL, x28_sigurg_sender, &sarg) != 0) {
        unlink(fifo_path);
        fail("pthread_create x28 openat sender");
    }
    rc = aarch64_syscall_with_x28(
        SYS_openat, AT_FDCWD, (long)fifo_path, O_RDONLY, 0, 0, &after_x28
    );
    g_x28_urg_done = 1;
    pthread_join(sender, NULL);
    unlink(fifo_path);
    if (rc != -EINTR || g_x28_in_sigurg != X28_SENTINEL || after_x28 != X28_SENTINEL) {
        fail("x28 corrupted during blocking openat SIGURG");
    }

    /* 3. Forwarded non-seccomp SIGSYS handler observes live x28 sentinel. */
    g_x28_in_sigsys = 0;
    rc = aarch64_syscall_with_x28(
        SYS_tgkill, (long)getpid(), (long)current_tid(), SIGSYS, 0, 0, &after_x28
    );
    if (rc != 0 || g_x28_in_sigsys != X28_SENTINEL || after_x28 != X28_SENTINEL) {
        fail("x28 corrupted during forwarded SIGSYS");
    }
#endif
    write_all(STDOUT_FILENO, "aarch64-x28-ok\n", 15);
    return 0;
}

/* ---- vfork-altstack-save subcommand (save_top != 0 & on_separate_altstack) ---- */

__attribute__((noinline))
static void clobber_stack_and_trap(const char *expected) {
    volatile unsigned char smash[4096];
    for (size_t i = 0; i < sizeof smash; i++) {
        smash[i] = 0x5a;
    }
    int fd = openat(AT_FDCWD, "/dev/null", O_RDONLY | O_CLOEXEC);
    if (fd < 0) {
        _exit(71);
    }
    close(fd);
    char buf[PATH_MAX];
    ssize_t n = readlink("/proc/self/exe", buf, sizeof buf - 1);
    if (n <= 0 || (size_t)n != strlen(expected) || memcmp(buf, expected, (size_t)n) != 0) {
        _exit(72);
    }
    if (smash[0] != 0x5a || smash[sizeof smash - 1] != 0x5a) {
        _exit(73);
    }
}

__attribute__((noinline))
static void run_raw_vfork_current_sp_once(const char *expected) {
    uint64_t g0 = UINT64_C(0x1020304050607080);
    uint64_t g1 = UINT64_C(0x90a0b0c0d0e0f001);
    double s0 = 1.25;
    double s1 = 2.5;
    double s2 = 3.75;
    double s3 = 4.125;
    double s4 = 5.625;
    double s5 = 6.875;
    double s6 = 7.375;
    double s7 = 8.9375;
#if defined(__aarch64__)
    __asm__ volatile(
        ""
        : "+r"(g0), "+r"(g1), "+w"(s0), "+w"(s1), "+w"(s2), "+w"(s3), "+w"(s4), "+w"(s5), "+w"(s6),
          "+w"(s7)
    );
    register long x8 __asm__("x8") = SYS_clone;
    register long x0 __asm__("x0") = (long)(CLONE_VM | CLONE_VFORK | SIGCHLD);
    register long x2 __asm__("x2") = 0;
    register long x3 __asm__("x3") = 0;
    register long x4 __asm__("x4") = 0;
    __asm__ volatile(
        "mov x1, sp\n\t"
        "svc #0\n\t"
        : "+r"(x0)
        : "r"(x8), "r"(x2), "r"(x3), "r"(x4)
        : "x1", "memory"
    );
    long pid = x0;
#elif defined(__x86_64__)
    __asm__ volatile(
        ""
        : "+r"(g0), "+r"(g1), "+x"(s0), "+x"(s1), "+x"(s2), "+x"(s3), "+x"(s4), "+x"(s5), "+x"(s6),
          "+x"(s7)
    );
    long pid;
    register long r10 __asm__("r10") = 0;
    register long r8 __asm__("r8") = 0;
    __asm__ volatile(
        "mov %%rsp, %%rsi\n\t"
        "syscall\n\t"
        : "=a"(pid)
        : "0"((long)SYS_clone),
          "D"((long)(CLONE_VM | CLONE_VFORK | SIGCHLD)),
          "d"(0L),
          "r"(r10),
          "r"(r8)
        : "rcx", "r11", "rsi", "memory"
    );
#else
#error "unsupported architecture"
#endif

    if (pid == 0) {
        clobber_stack_and_trap(expected);
        _exit(0);
    }
    if (pid < 0) {
        fail("raw SYS_clone(CLONE_VM | CLONE_VFORK, current_sp) failed");
    }

    int status = 0;
    if (waitpid((pid_t)pid, &status, 0) != (pid_t)pid || !WIFEXITED(status)
        || WEXITSTATUS(status) != 0) {
        fail("raw current_sp vfork child failed");
    }
    if (g0 != UINT64_C(0x1020304050607080) || g1 != UINT64_C(0x90a0b0c0d0e0f001)
        || s0 != 1.25 || s1 != 2.5 || s2 != 3.75 || s3 != 4.125
        || s4 != 5.625 || s5 != 6.875 || s6 != 7.375 || s7 != 8.9375) {
        fail("parent locals or FP/SIMD regs corrupted by current_sp vfork child");
    }
}

static void *g_separate_vfork_stack_top = NULL;
static volatile sig_atomic_t g_altstack_vfork_ok = 0;

static void altstack_vfork_usr1_handler(int sig) {
    if (sig != SIGUSR1) {
        return;
    }
    pid_t pid = clone(
        tiny_vfork_child_fn,
        g_separate_vfork_stack_top,
        CLONE_VM | CLONE_VFORK | SIGCHLD,
        (void *)g_expected_exe
    );
    if (pid > 0) {
        int status = 0;
        if (waitpid(pid, &status, 0) == pid && WIFEXITED(status) && WEXITSTATUS(status) == 42) {
            g_altstack_vfork_ok = 1;
        }
    }
}

static int run_vfork_altstack_save_test(void) {
    const char *expected = expected_exe_path();
    g_expected_exe = expected;

    /* 1. Exercise save_top != 0 path via raw clone(CLONE_VM | CLONE_VFORK, current_sp). */
    for (int i = 0; i < 4; i++) {
        run_raw_vfork_current_sp_once(expected);
    }

    /* 2. Exercise on_separate_altstack == true by running a separate-stack
     *    CLONE_VM | CLONE_VFORK clone from inside an SA_ONSTACK signal handler. */
    size_t alt_sz = 64 * 1024;
    unsigned char *alt_mem =
        mmap(NULL, alt_sz, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    unsigned char *child_mem =
        mmap(NULL, 16 * 1024, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (alt_mem == MAP_FAILED || child_mem == MAP_FAILED) {
        fail("mmap altstack/child_stack for vfork");
    }
    g_separate_vfork_stack_top = (void *)(((uintptr_t)(child_mem + 16 * 1024)) & ~(uintptr_t)15);

    stack_t ss;
    ss.ss_sp = alt_mem;
    ss.ss_size = alt_sz;
    ss.ss_flags = 0;
    if (sigaltstack(&ss, NULL) != 0) {
        fail("sigaltstack vfork");
    }
    struct sigaction sa;
    memset(&sa, 0, sizeof sa);
    sa.sa_handler = altstack_vfork_usr1_handler;
    sa.sa_flags = SA_ONSTACK;
    sigemptyset(&sa.sa_mask);
    if (sigaction(SIGUSR1, &sa, NULL) != 0) {
        fail("sigaction SIGUSR1 altstack vfork");
    }
    g_altstack_vfork_ok = 0;
    if (raise(SIGUSR1) != 0 || !g_altstack_vfork_ok) {
        fail("vfork from SA_ONSTACK user altstack failed");
    }

    ss.ss_flags = SS_DISABLE;
    sigaltstack(&ss, NULL);
    munmap(alt_mem, alt_sz);
    munmap(child_mem, 16 * 1024);

    write_all(STDOUT_FILENO, "vfork-altstack-save-ok\n", 23);
    return 0;
}

/* ---- stack-pool-overflow subcommand (> 64 concurrent trapping threads) ---- */

struct pool_overflow_arg {
    pthread_barrier_t *barrier;
    const char *expected;
};

static void *pool_overflow_worker(void *raw) {
    struct pool_overflow_arg *arg = (struct pool_overflow_arg *)raw;
    pthread_barrier_wait(arg->barrier);
    for (int i = 0; i < 40; i++) {
        int fd = openat(AT_FDCWD, "/dev/null", O_RDONLY | O_CLOEXEC);
        if (fd < 0) {
            fail("pool overflow openat");
        }
        close(fd);
        assert_readlink_matches_expected("/proc/self/exe", arg->expected);
    }
    return NULL;
}

static int run_stack_pool_overflow_test(void) {
    enum { NUM_THREADS = 100 };
    const char *expected = expected_exe_path();
    pthread_t threads[NUM_THREADS];
    pthread_barrier_t barrier;
    if (pthread_barrier_init(&barrier, NULL, NUM_THREADS) != 0) {
        fail("pthread_barrier_init pool overflow");
    }
    pthread_attr_t attr;
    if (pthread_attr_init(&attr) != 0) {
        fail("pthread_attr_init pool overflow");
    }
    (void)pthread_attr_setstacksize(&attr, 64 * 1024);

    struct pool_overflow_arg arg = {&barrier, expected};
    for (int i = 0; i < NUM_THREADS; i++) {
        if (pthread_create(&threads[i], &attr, pool_overflow_worker, &arg) != 0) {
            fail("pthread_create pool overflow");
        }
    }
    for (int i = 0; i < NUM_THREADS; i++) {
        if (pthread_join(threads[i], NULL) != 0) {
            fail("pthread_join pool overflow");
        }
    }
    pthread_attr_destroy(&attr);
    pthread_barrier_destroy(&barrier);

    write_all(STDOUT_FILENO, "stack-pool-overflow-ok\n", 23);
    return 0;
}

/* ---- thread-exit-tid-cleanup subcommand ---- */

static void *exit_blocking_all_thread_fn(void *unused) {
    (void)unused;
    sigset_t all;
    sigfillset(&all);
    if (pthread_sigmask(SIG_SETMASK, &all, NULL) != 0) {
        fail("exit_blocking_all_thread_fn block all");
    }
    return NULL;
}

static void *verify_clean_sigsys_thread_fn(void *unused) {
    (void)unused;
    sigset_t cur, only_sigsys;
    sigemptyset(&cur);
    if (pthread_sigmask(SIG_SETMASK, NULL, &cur) != 0 || sigismember(&cur, SIGSYS) != 0) {
        fail("subsequent thread observed stale blocked SIGSYS");
    }

    g_sigsys_plain_count = 0;
    if (pthread_kill(pthread_self(), SIGSYS) != 0 || g_sigsys_plain_count != 1) {
        fail("subsequent thread did not deliver SIGSYS immediately");
    }

    sigemptyset(&only_sigsys);
    sigaddset(&only_sigsys, SIGSYS);
    if (pthread_sigmask(SIG_BLOCK, &only_sigsys, NULL) != 0) {
        fail("subsequent thread block SIGSYS");
    }
    if (pthread_kill(pthread_self(), SIGSYS) != 0 || g_sigsys_plain_count != 1) {
        fail("subsequent thread did not defer blocked SIGSYS");
    }
    if (pthread_sigmask(SIG_UNBLOCK, &only_sigsys, NULL) != 0 || g_sigsys_plain_count != 2) {
        fail("subsequent thread did not deliver deferred SIGSYS on unblock");
    }
    return NULL;
}

static int run_thread_exit_tid_cleanup_test(void) {
    g_expected_exe = expected_exe_path();
    struct sigaction act;
    memset(&act, 0, sizeof act);
    act.sa_handler = sigsys_plain_handler;
    sigemptyset(&act.sa_mask);
    if (sigaction(SIGSYS, &act, NULL) != 0) {
        fail("install sigsys handler");
    }

    pthread_attr_t attr;
    if (pthread_attr_init(&attr) != 0) {
        fail("pthread_attr_init");
    }
    (void)pthread_attr_setstacksize(&attr, 64 * 1024);

    for (int round = 0; round < 32; round++) {
        for (int i = 0; i < 4; i++) {
            pthread_t t;
            if (pthread_create(&t, &attr, exit_blocking_all_thread_fn, NULL) != 0) {
                fail("pthread_create exit_blocking_all");
            }
            if (pthread_join(t, NULL) != 0) {
                fail("pthread_join exit_blocking_all");
            }
        }
        pthread_t verify;
        if (pthread_create(&verify, &attr, verify_clean_sigsys_thread_fn, NULL) != 0) {
            fail("pthread_create verify_clean_sigsys");
        }
        if (pthread_join(verify, NULL) != 0) {
            fail("pthread_join verify_clean_sigsys");
        }
    }

    pthread_attr_destroy(&attr);
    write_all(STDOUT_FILENO, "thread-exit-tid-cleanup-ok\n", 27);
    return 0;
}

/* ---- posix-spawn-sigsys-dfl subcommand ---- */

static int vfork_sigsys_override_child_fn(void *unused) {
    (void)unused;
    struct kernel_sigaction_compat {
        void (*sa_handler_fn)(int);
        unsigned long sa_flags;
        void (*sa_restorer_fn)(void);
        uint64_t sa_mask;
    };
    struct kernel_sigaction_compat old_act, new_act;
    memset(&old_act, 0, sizeof old_act);
    if (syscall(SYS_rt_sigaction, SIGSYS, NULL, &old_act, sizeof(uint64_t)) != 0) {
        _exit(10);
    }
    if (old_act.sa_handler_fn != SIG_IGN) {
        _exit(11);
    }

    /* Reset SIGSYS to SIG_DFL inside the CLONE_VM child. */
    memset(&new_act, 0, sizeof new_act);
    new_act.sa_handler_fn = SIG_DFL;
    memset(&old_act, 0, sizeof old_act);
    if (syscall(SYS_rt_sigaction, SIGSYS, &new_act, &old_act, sizeof(uint64_t)) != 0) {
        _exit(12);
    }
    if (old_act.sa_handler_fn != SIG_IGN) {
        _exit(13);
    }

    /* Mutate sigmask (like posix_spawn POSIX_SPAWN_SETSIGMASK) and confirm
     * BIT_CHILD_SIGSYS_DFL was not wiped! */
    uint64_t block_sigsys = UINT64_C(1) << (SIGSYS - 1);
    uint64_t empty_mask = 0;
    if (syscall(SYS_rt_sigprocmask, SIG_SETMASK, &block_sigsys, NULL, sizeof(uint64_t)) != 0) {
        _exit(14);
    }
    if (syscall(SYS_rt_sigprocmask, SIG_SETMASK, &empty_mask, NULL, sizeof(uint64_t)) != 0) {
        _exit(15);
    }

    memset(&old_act, 0, sizeof old_act);
    if (syscall(SYS_rt_sigaction, SIGSYS, NULL, &old_act, sizeof(uint64_t)) != 0) {
        _exit(16);
    }
    if (old_act.sa_handler_fn != SIG_DFL) {
        _exit(17);
    }

    /* Switch back to SIG_IGN and verify readback. */
    new_act.sa_handler_fn = SIG_IGN;
    memset(&old_act, 0, sizeof old_act);
    if (syscall(SYS_rt_sigaction, SIGSYS, &new_act, &old_act, sizeof(uint64_t)) != 0) {
        _exit(18);
    }
    if (old_act.sa_handler_fn != SIG_DFL) {
        _exit(19);
    }
    memset(&old_act, 0, sizeof old_act);
    if (syscall(SYS_rt_sigaction, SIGSYS, NULL, &old_act, sizeof(uint64_t)) != 0) {
        _exit(20);
    }
    if (old_act.sa_handler_fn != SIG_IGN) {
        _exit(21);
    }

    _exit(0);
}

static int run_posix_spawn_sigsys_dfl_test(void) {
    struct sigaction ign, old;
    memset(&ign, 0, sizeof ign);
    ign.sa_handler = SIG_IGN;
    sigemptyset(&ign.sa_mask);
    if (sigaction(SIGSYS, &ign, NULL) != 0) {
        fail("parent set SIGSYS SIG_IGN");
    }

    /* 1. Exercise direct CLONE_VM | CLONE_VFORK child override + sigprocmask
     *    survival + clear_pid_slots on _exit(0) without exec. */
    size_t stack_sz = 64 * 1024;
    void *child_stack = mmap(NULL, stack_sz, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (child_stack == MAP_FAILED) {
        fail("mmap vfork child stack");
    }
    pid_t vpid = clone(
        vfork_sigsys_override_child_fn,
        (char *)child_stack + stack_sz,
        CLONE_VM | CLONE_VFORK | SIGCHLD,
        NULL
    );
    if (vpid < 0) {
        fail("clone CLONE_VM|CLONE_VFORK");
    }
    int vstatus = 0;
    if (waitpid(vpid, &vstatus, 0) != vpid || !WIFEXITED(vstatus) || WEXITSTATUS(vstatus) != 0) {
        fail("vfork_sigsys_override_child_fn failed");
    }
    munmap(child_stack, stack_sz);

    /* 2. Under --test-interceptor, probe `call.sigsys_ignored()` during real
     *    `posix_spawn` with `POSIX_SPAWN_SETSIGDEF | POSIX_SPAWN_SETSIGMASK`. */
    if (getenv("FSPY_TEST_INTERCEPTOR_ACTIVE") != NULL) {
        char *probe_argv[] = {"probe", NULL};
        sigset_t full;
        sigfillset(&full);

        /* 2a. With POSIX_SPAWN_SETSIGDEF containing SIGSYS AND POSIX_SPAWN_SETSIGMASK:
         *     interceptor must observe `sigsys_ignored() == false` -> ENOENT. */
        posix_spawnattr_t attr_dfl;
        if (posix_spawnattr_init(&attr_dfl) != 0) {
            fail("posix_spawnattr_init dfl");
        }
        if (posix_spawnattr_setflags(&attr_dfl, POSIX_SPAWN_SETSIGDEF | POSIX_SPAWN_SETSIGMASK) != 0
            || posix_spawnattr_setsigdefault(&attr_dfl, &full) != 0
            || posix_spawnattr_setsigmask(&attr_dfl, &full) != 0) {
            fail("posix_spawnattr configure dfl");
        }
        pid_t pid = -1;
        int rc = posix_spawn(&pid, "/fspy_probe_sigsys_ignored", NULL, &attr_dfl, probe_argv, environ);
        posix_spawnattr_destroy(&attr_dfl);
        if (rc != ENOENT) {
            errno = rc;
            fail("expected ENOENT (sigsys_ignored=false) with POSIX_SPAWN_SETSIGDEF");
        }

        /* 2b. Without POSIX_SPAWN_SETSIGDEF (only POSIX_SPAWN_SETSIGMASK):
         *     interceptor must observe `sigsys_ignored() == true` -> ELOOP. */
        posix_spawnattr_t attr_ign;
        if (posix_spawnattr_init(&attr_ign) != 0) {
            fail("posix_spawnattr_init ign");
        }
        if (posix_spawnattr_setflags(&attr_ign, POSIX_SPAWN_SETSIGMASK) != 0
            || posix_spawnattr_setsigmask(&attr_ign, &full) != 0) {
            fail("posix_spawnattr configure ign");
        }
        pid = -1;
        rc = posix_spawn(&pid, "/fspy_probe_sigsys_ignored", NULL, &attr_ign, probe_argv, environ);
        posix_spawnattr_destroy(&attr_ign);
        if (rc != ELOOP) {
            errno = rc;
            fail("expected ELOOP (sigsys_ignored=true) without POSIX_SPAWN_SETSIGDEF");
        }
    }

    /* Parent must STILL have SIGSYS == SIG_IGN! */
    memset(&old, 0, sizeof old);
    if (sigaction(SIGSYS, NULL, &old) != 0 || old.sa_handler != SIG_IGN) {
        fail("parent lost SIGSYS SIG_IGN after CLONE_VM child override");
    }
    if (raise(SIGSYS) != 0) {
        fail("parent raise(SIGSYS) failed under retained SIG_IGN");
    }

    write_all(STDOUT_FILENO, "posix-spawn-sigsys-dfl-ok\n", 26);
    return 0;
}

static int run_x32_enosys_test(void) {
#ifdef __x86_64__
    errno = 0;
    long rc = syscall(0x40000000L | SYS_getpid);
    if (rc != -1 || errno != ENOSYS) {
        fail("expected x32 getpid syscall to return -1/ENOSYS");
    }
#endif
    write_all(STDOUT_FILENO, "x32-enosys-ok\n", 14);
    return 0;
}

static int run_validate_exec_eexist_test(void) {
    const char *overlap_path = getenv("OVERLAP_ELF_PATH");
    if (overlap_path == NULL || overlap_path[0] == '\0') {
        fail("missing OVERLAP_ELF_PATH");
    }
    errno = 0;
    long rc = syscall(SYS_execve, overlap_path, (void *)0x55553456UL, NULL);
    if (rc != -1 || errno != EEXIST) {
        fail("expected intercepted overlapping ET_EXEC to fail with EEXIST");
    }
    write_all(STDOUT_FILENO, "validate-exec-eexist-ok\n", 24);
    return 0;
}

int main(int argc, char **argv) {
    if (argc < 2) {
        fail("missing subcommand");
    }
    if (strcmp(argv[1], "reexec-exit-73") == 0) {
        _exit(73);
    }
    if (strcmp(argv[1], "sigaction") == 0) {
        return run_sigaction_test();
    }
    if (strcmp(argv[1], "sigsys-default-death") == 0) {
        return run_sigsys_default_death();
    }
    if (strcmp(argv[1], "sigmask-single") == 0) {
        return run_sigmask_single_test();
    }
    if (strcmp(argv[1], "sigmask-threads") == 0) {
        return run_sigmask_threads_test();
    }
    if (strcmp(argv[1], "proc-exe") == 0) {
        return run_proc_exe_test();
    }
    if (strcmp(argv[1], "comm") == 0) {
        return run_comm_test();
    }
    if (strcmp(argv[1], "deep-stack") == 0) {
        return run_deep_stack_test();
    }
    if (strcmp(argv[1], "interceptor") == 0) {
        return run_interceptor_test();
    }
    if (strcmp(argv[1], "recursive-exec") == 0) {
        return run_recursive_exec_test();
    }
    if (strcmp(argv[1], "nested-seccomp") == 0) {
        return run_nested_seccomp_test();
    }
    if (strcmp(argv[1], "posix-spawn") == 0) {
        return run_posix_spawn_test();
    }
    if (strcmp(argv[1], "clone3-enosys") == 0) {
        return run_clone3_enosys_test();
    }
    if (strcmp(argv[1], "sigaltstack-storm") == 0) {
        return run_sigaltstack_storm_test();
    }
    if (strcmp(argv[1], "fifo-open-eintr") == 0) {
        return run_fifo_open_eintr_test();
    }
    if (strcmp(argv[1], "blocked-sigsys-pending") == 0) {
        return run_blocked_sigsys_pending_test();
    }
    if (strcmp(argv[1], "wait-mask-sigsys") == 0) {
        return run_wait_mask_sigsys_test();
    }
    if (strcmp(argv[1], "tid-churn") == 0) {
        return run_tid_churn_test();
    }
    if (strcmp(argv[1], "fork-sigmask-and-lock") == 0) {
        return run_fork_sigmask_and_lock_test();
    }
    if (strcmp(argv[1], "vfork-parent-isolation") == 0) {
        return run_vfork_parent_isolation_test();
    }
    if (strcmp(argv[1], "tiny-vfork-stack") == 0) {
        return run_tiny_vfork_stack_test();
    }
    if (strcmp(argv[1], "abseil-address-probe") == 0) {
        return run_abseil_address_probe_test();
    }
    if (strcmp(argv[1], "vfork-shared-stack-deep") == 0) {
        return run_vfork_shared_stack_deep_test();
    }
    if (strcmp(argv[1], "clone-custom-stack-new-mm") == 0) {
        return run_clone_custom_stack_new_mm_test();
    }
    if (strcmp(argv[1], "pselect-timeout-deferred-sigsys") == 0) {
        return run_pselect_timeout_deferred_sigsys_test();
    }
    if (strcmp(argv[1], "proc-sibling-tid-exe") == 0) {
        return run_proc_sibling_tid_exe_test();
    }
    if (strcmp(argv[1], "altstack-8kib-storm") == 0) {
        return run_altstack_8kib_storm_test();
    }
    if (strcmp(argv[1], "pselect-simultaneous-eintr") == 0) {
        return run_pselect_simultaneous_eintr_test();
    }
    if (strcmp(argv[1], "posix-spawnp-long-path") == 0) {
        return run_posix_spawnp_long_path_test();
    }
    if (strcmp(argv[1], "io-uring-enosys") == 0) {
        return run_io_uring_enosys_test();
    }
    if (strcmp(argv[1], "inherited-sigsys") == 0) {
        return run_inherited_sigsys_test();
    }
    if (strcmp(argv[1], "aarch64-x28") == 0) {
        return run_aarch64_x28_test();
    }
    if (strcmp(argv[1], "vfork-altstack-save") == 0) {
        return run_vfork_altstack_save_test();
    }
    if (strcmp(argv[1], "stack-pool-overflow") == 0) {
        return run_stack_pool_overflow_test();
    }
    if (strcmp(argv[1], "thread-exit-tid-cleanup") == 0) {
        return run_thread_exit_tid_cleanup_test();
    }
    if (strcmp(argv[1], "posix-spawn-sigsys-dfl") == 0) {
        return run_posix_spawn_sigsys_dfl_test();
    }
    if (strcmp(argv[1], "x32-enosys") == 0) {
        return run_x32_enosys_test();
    }
    if (strcmp(argv[1], "validate-exec-eexist") == 0) {
        return run_validate_exec_eexist_test();
    }
    fail("unknown subcommand");
    return 1;
}
