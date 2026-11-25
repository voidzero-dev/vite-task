/* Runtime checks for all four Linux ELF64 cells loaded by exec_elf:
 * dynamic PIE, dynamic ET_EXEC, traditional static, and static-PIE.
 *
 * Protocol: argv[0]="custom-argv-zero", argv[1]="", argv[2]=0xff,
 * EMPTY="", BYTES=0xff, FIXTURE_IMAGE=AT_EXECFN, stdout "runtime-ok\n",
 * exit 37. Set EXPECT_INTERPRETER=1 when PT_INTERP is present (AT_BASE != 0)
 * and EXPECT_FIXED_ADDRESS=1 for ET_EXEC images (load bias == 0).
 */

#define _GNU_SOURCE

#include <dirent.h>
#include <elf.h>
#include <limits.h>
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/auxv.h>
#include <unistd.h>

static char bss[128 * 1024];
static int data = 42;
static __thread int tls = 17;

static void write_all(int fd, const char *buf, size_t len) {
    while (len > 0) {
        ssize_t written = write(fd, buf, len);
        if (written <= 0) {
            return;
        }
        buf += written;
        len -= (size_t)written;
    }
}

static void fail(const char *message) {
    write_all(STDERR_FILENO, message, strlen(message));
    write_all(STDERR_FILENO, "\n", 1);
    _exit(2);
}

static void *thread_main(void *argument) {
    if (tls != 17) {
        fail("thread tls");
    }
    *(int *)argument = 1;
    return NULL;
}

static unsigned long load_bias(unsigned long at_phdr, unsigned long phnum, const Elf64_Phdr *ph) {
    unsigned long i;
    for (i = 0; i < phnum; i++) {
        if (ph[i].p_type == PT_PHDR) {
            return at_phdr - (unsigned long)ph[i].p_vaddr;
        }
    }
    /* GNU ld omits PT_PHDR on some static images. ELF64 headers start at 0. */
    const Elf64_Ehdr *ehdr = (const Elf64_Ehdr *)(at_phdr - sizeof *ehdr);
    if (memcmp(ehdr->e_ident, ELFMAG, SELFMAG) != 0) {
        fail("elf header");
    }
    if (ehdr->e_phoff != sizeof *ehdr) {
        fail("e_phoff");
    }
    for (i = 0; i < phnum; i++) {
        if (ph[i].p_type == PT_LOAD && ph[i].p_offset == 0) {
            return at_phdr - (unsigned long)ehdr->e_phoff - (unsigned long)ph[i].p_vaddr;
        }
    }
    fail("no load at file start");
    return 0;
}

int main(int argc, char **argv) {
    unsigned long i;
    pthread_t thread;
    int started = 0;
    char *heap;
    unsigned long at_base;
    unsigned long at_phdr;
    unsigned long phnum;
    unsigned long phent;
    unsigned long entry;
    unsigned long bias;
    const Elf64_Phdr *ph;
    const char *image;

    if (argc != 3) {
        fail("argc");
    }
    if (strcmp(argv[0], "custom-argv-zero") != 0) {
        fail("argv0");
    }
    if (argv[1][0] != '\0') {
        fail("argv1");
    }
    if ((unsigned char)argv[2][0] != 0xff || argv[2][1] != '\0') {
        fail("argv2");
    }
    if (getenv("EMPTY") == NULL || getenv("EMPTY")[0] != '\0') {
        fail("EMPTY");
    }
    {
        const char *bytes = getenv("BYTES");
        if (bytes == NULL || (unsigned char)bytes[0] != 0xff || bytes[1] != '\0') {
            fail("BYTES");
        }
    }
    for (i = 0; i < sizeof bss; i++) {
        if (bss[i] != 0) {
            fail("bss");
        }
    }
    bss[sizeof bss - 1] = 9;
    if (data != 42) {
        fail("data");
    }
    data = 43;
    if (tls != 17) {
        fail("tls");
    }
    tls = 23;
    if (pthread_create(&thread, NULL, thread_main, &started) != 0) {
        fail("pthread_create");
    }
    if (pthread_join(thread, NULL) != 0 || started != 1) {
        fail("pthread_join");
    }
    if (tls != 23) {
        fail("tls after join");
    }

    heap = malloc(128 * 1024);
    if (heap == NULL) {
        fail("malloc");
    }
    memset(heap, 7, 128 * 1024);
    for (i = 0; i < 128 * 1024; i++) {
        if ((unsigned char)heap[i] != 7) {
            fail("heap");
        }
    }
    free(heap);

    at_base = getauxval(AT_BASE);
    if (getenv("EXPECT_INTERPRETER") != NULL) {
        const unsigned char *interp = (const unsigned char *)at_base;
        if (at_base == 0 || memcmp(interp, "\x7f" "ELF", 4) != 0) {
            fail("AT_BASE interpreter");
        }
    } else if (at_base != 0) {
        fail("AT_BASE");
    }

    at_phdr = getauxval(AT_PHDR);
    phnum = getauxval(AT_PHNUM);
    phent = getauxval(AT_PHENT);
    entry = getauxval(AT_ENTRY);
    if (phent != sizeof(Elf64_Phdr) || at_phdr == 0 || phnum == 0 || entry == 0) {
        fail("auxv");
    }
    ph = (const Elf64_Phdr *)at_phdr;
    bias = load_bias(at_phdr, phnum, ph);
    if (getenv("EXPECT_FIXED_ADDRESS") != NULL && bias != 0) {
        fail("relocated fixed image");
    }
    {
        int found_entry = 0;
        for (i = 0; i < phnum; i++) {
            unsigned long start;
            unsigned long end;
            if (ph[i].p_type != PT_LOAD || (ph[i].p_flags & PF_X) == 0) {
                continue;
            }
            start = bias + (unsigned long)ph[i].p_vaddr;
            end = start + (unsigned long)ph[i].p_memsz;
            if (entry >= start && entry < end) {
                found_entry = 1;
            }
        }
        if (!found_entry) {
            fail("AT_ENTRY");
        }
    }
    image = getenv("FIXTURE_IMAGE");
    if (image == NULL || strcmp((const char *)getauxval(AT_EXECFN), image) != 0) {
        fail("AT_EXECFN");
    }
    {
        const unsigned char *random = (const unsigned char *)getauxval(AT_RANDOM);
        int nonzero = 0;
        if (random == NULL) {
            fail("AT_RANDOM");
        }
        for (i = 0; i < 16; i++) {
            if (random[i] != 0) {
                nonzero = 1;
            }
        }
        if (!nonzero) {
            fail("AT_RANDOM zero");
        }
    }

    {
        DIR *dir = opendir("/proc/self/fd");
        struct dirent *entry;
        if (dir == NULL) {
            fail("opendir /proc/self/fd");
        }
        while ((entry = readdir(dir)) != NULL) {
            char link_path[PATH_MAX];
            char target[PATH_MAX];
            ssize_t len;
            if (entry->d_name[0] == '.') {
                continue;
            }
            snprintf(link_path, sizeof link_path, "/proc/self/fd/%s", entry->d_name);
            len = readlink(link_path, target, sizeof target - 1);
            if (len >= 0) {
                target[len] = '\0';
                if (strcmp(target, image) == 0) {
                    fail("executable fd leaked");
                }
            }
        }
        closedir(dir);
    }

    write_all(STDOUT_FILENO, "runtime-ok\n", 11);
    _exit(37);
}
