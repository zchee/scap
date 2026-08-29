/* Filesystem-syscall counter for the ADR-8 budget (plan
 * docs/plans/2026-08-28-theoretical-limit-optimization.md, ADR-8 "Syscall
 * budget"; W2.1).
 *
 * `fs_usage -w -f filesys` and `ktrace trace` both refuse to run as anything
 * but root, and the reference machine's `sudo` is interactive-only, so the
 * counts are taken from inside the process instead: this is a dyld
 * interposing library that wraps the libc entry points scap's configuration
 * path can reach and writes the totals when the process exits.
 *
 * What it counts, and only this: calls to the nine interposed symbols
 * `open`, `openat`, `stat`, `lstat`, `fstat`, `fstatat`, `access`,
 * `getcwd` and `readlink`. It is therefore a LOWER BOUND on filesystem
 * syscalls, not a trace: `opendir`/`readdir`, `getattrlist`/
 * `getattrlistbulk`, and the `lstat` chain `realpath(3)` runs inside libc
 * (which does not route back through the interposed symbol) are all
 * invisible to it. dyld's own startup work IS included, so
 * scripts/syscall-count.sh subtracts the figure an empty `fn main` binary
 * produces under the same library.
 *
 * On arm64 the file-status symbols carry no `$INODE64` suffix, so
 * interposing `stat`/`fstat`/`lstat` by name is exact. A binary running
 * under Rosetta resolves the legacy `stat$INODE64` aliases instead and
 * would slip past these wrappers; measure native builds only.
 *
 * Build and drive it with scripts/syscall-count.sh; it is a measurement
 * tool, never linked into scap.
 *
 * Env:
 *   SYSCALL_COUNT_OUT    append the summary line here instead of stderr.
 *   SYSCALL_COUNT_TRACE  also append one `<call> <path>` line per call here.
 */

#include <fcntl.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/stat.h>
#include <unistd.h>

static int n_open, n_openat, n_stat, n_lstat, n_fstat, n_fstatat, n_access, n_getcwd,
    n_readlink;

/* Re-entrancy guard. Writing the trace opens a file, which re-enters the
 * interposed `open` -- unguarded that recurses until the stack runs out, and
 * it would also count the library's own bookkeeping. Every wrapper both
 * skips counting and skips tracing while this is set.
 *
 * Deliberately NOT `__thread`: the wrappers fire during dyld startup, before
 * thread-local storage is initialised, and a TLV access there aborts. A
 * plain static is safe for what this tool measures -- a short, effectively
 * single-threaded configuration path -- and the worst a race could do is
 * miscount, never crash. */
static int in_hook;

__attribute__((destructor)) static void report(void) {
    in_hook = 1; /* writing the summary must not count itself */
    const char *out = getenv("SYSCALL_COUNT_OUT");
    FILE *f = out ? fopen(out, "a") : stderr;
    if (!f) {
        return;
    }
    int total = n_open + n_openat + n_stat + n_lstat + n_fstat + n_fstatat + n_access +
                n_getcwd + n_readlink;
    fprintf(f,
            "open=%d openat=%d stat=%d lstat=%d fstat=%d fstatat=%d access=%d getcwd=%d "
            "readlink=%d TOTAL=%d\n",
            n_open, n_openat, n_stat, n_lstat, n_fstat, n_fstatat, n_access, n_getcwd,
            n_readlink, total);
    if (out) {
        fclose(f);
    }
}

/* The trace writes with a raw descriptor and `write(2)` rather than stdio.
 * These wrappers fire during dyld startup, before stdio is initialised, and
 * `fopen` there crashes; `open` once plus `write` does not. */
static int trace_fd = -2; /* -2 not opened yet, -1 disabled */

static void trace(const char *call, const char *path) {
    if (in_hook) {
        return;
    }
    in_hook = 1;
    if (trace_fd == -2) {
        const char *out = getenv("SYSCALL_COUNT_TRACE");
        /* dyld does not rebind the image that defines an interposition, so
         * this reaches the real `open`, not the wrapper below. */
        trace_fd = out ? open(out, O_WRONLY | O_CREAT | O_APPEND, 0644) : -1;
    }
    if (trace_fd >= 0) {
        char line[4096];
        int len = snprintf(line, sizeof line, "%s %s\n", call, path ? path : "(null)");
        if (len > 0) {
            size_t bytes = (size_t)len < sizeof line ? (size_t)len : sizeof line - 1;
            ssize_t written = write(trace_fd, line, bytes);
            (void)written; /* a truncated trace must not disturb the run */
        }
    }
    in_hook = 0;
}

/* Count one call unless we are inside our own bookkeeping. */
#define COUNT(counter, call, path)                                                            \
    do {                                                                                      \
        if (!in_hook) {                                                                       \
            (counter)++;                                                                      \
            trace((call), (path));                                                            \
        }                                                                                     \
    } while (0)

#define DYLD_INTERPOSE(_repl, _orig)                                                          \
    __attribute__((used)) static struct {                                                     \
        const void *r;                                                                        \
        const void *o;                                                                        \
    } _interpose_##_orig __attribute__((section("__DATA,__interpose"))) = {                   \
        (const void *)(unsigned long)&_repl, (const void *)(unsigned long)&_orig};

/* `open`/`openat` are variadic: the third argument is the creation mode and
 * is present exactly when O_CREAT (or O_TMPFILE) is set. Dropping it would
 * create files with whatever happened to be in the register, so it is read
 * with va_arg and passed on. */
static int counted_open(const char *path, int flags, ...) {
    COUNT(n_open, "open", path);
    if (flags & O_CREAT) {
        va_list args;
        va_start(args, flags);
        mode_t mode = (mode_t)va_arg(args, int);
        va_end(args);
        return open(path, flags, mode);
    }
    return open(path, flags);
}

static int counted_openat(int dirfd, const char *path, int flags, ...) {
    COUNT(n_openat, "openat", path);
    if (flags & O_CREAT) {
        va_list args;
        va_start(args, flags);
        mode_t mode = (mode_t)va_arg(args, int);
        va_end(args);
        return openat(dirfd, path, flags, mode);
    }
    return openat(dirfd, path, flags);
}

static int counted_stat(const char *path, struct stat *out) {
    COUNT(n_stat, "stat", path);
    return stat(path, out);
}

static int counted_lstat(const char *path, struct stat *out) {
    COUNT(n_lstat, "lstat", path);
    return lstat(path, out);
}

static int counted_fstat(int fd, struct stat *out) {
    COUNT(n_fstat, "fstat", "(fd)");
    return fstat(fd, out);
}

static int counted_fstatat(int dirfd, const char *path, struct stat *out, int flags) {
    COUNT(n_fstatat, "fstatat", path);
    return fstatat(dirfd, path, out, flags);
}

static int counted_access(const char *path, int mode) {
    COUNT(n_access, "access", path);
    return access(path, mode);
}

static char *counted_getcwd(char *buf, size_t size) {
    COUNT(n_getcwd, "getcwd", "");
    return getcwd(buf, size);
}

static ssize_t counted_readlink(const char *path, char *buf, size_t size) {
    COUNT(n_readlink, "readlink", path);
    return readlink(path, buf, size);
}

DYLD_INTERPOSE(counted_open, open)
DYLD_INTERPOSE(counted_openat, openat)
DYLD_INTERPOSE(counted_stat, stat)
DYLD_INTERPOSE(counted_lstat, lstat)
DYLD_INTERPOSE(counted_fstat, fstat)
DYLD_INTERPOSE(counted_fstatat, fstatat)
DYLD_INTERPOSE(counted_access, access)
DYLD_INTERPOSE(counted_getcwd, getcwd)
DYLD_INTERPOSE(counted_readlink, readlink)
