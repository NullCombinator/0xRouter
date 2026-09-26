/* 0router-gate — Landlock filesystem confinement for the 0router identity.
 * Usage: 0router-gate rw:PATH,ro:PATH[,more] -- prog [args...]
 * Fail-closed: any setup error exits 126 without running the target. */
#define _GNU_SOURCE 1
#include <errno.h>
#include <fcntl.h>
#include <linux/landlock.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/prctl.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <unistd.h>

#ifndef landlock_create_ruleset
#define landlock_create_ruleset(attr, size, flags) syscall(__NR_landlock_create_ruleset, attr, size, flags)
#endif
#ifndef landlock_add_rule
#define landlock_add_rule(fd, type, attr, flags) syscall(__NR_landlock_add_rule, fd, type, attr, flags)
#endif
#ifndef landlock_restrict_self
#define landlock_restrict_self(fd, flags) syscall(__NR_landlock_restrict_self, fd, flags)
#endif

int rule_add(int rfd, const char *path, unsigned long acc) {
  struct landlock_path_beneath_attr pb;
  struct stat st;
  int pfd = open(path, O_PATH | O_CLOEXEC);
  if (pfd < 0) { fprintf(stderr, "0router-gate: open %s: %s\n", path, strerror(errno)); return -1; }
  if (fstat(pfd, &st) == 0 && !S_ISDIR(st.st_mode)) {
    /* Landlock: non-directory fds accept only file-level rights. */
    unsigned long FILE_ACC = LANDLOCK_ACCESS_FS_EXECUTE | LANDLOCK_ACCESS_FS_WRITE_FILE |
        LANDLOCK_ACCESS_FS_READ_FILE | LANDLOCK_ACCESS_FS_TRUNCATE;
    /* Char devices ruled rw (ptmx, tty, null) need ioctls — PTY allocation
     * is pure ioctl on /dev/ptmx. Grant the ABI-5 ioctl right to rw rules. */
#ifdef LANDLOCK_ACCESS_FS_IOCTL_DEV
    FILE_ACC |= LANDLOCK_ACCESS_FS_IOCTL_DEV;
#elif defined(LANDLOCK_ACCESS_FS_IOCTL)
    FILE_ACC |= LANDLOCK_ACCESS_FS_IOCTL;
#endif
    acc &= FILE_ACC;
  }
  pb.allowed_access = acc;
  pb.parent_fd = pfd;
  if (landlock_add_rule(rfd, LANDLOCK_RULE_PATH_BENEATH, &pb, 0)) {
    fprintf(stderr, "0router-gate: add_rule %s: %s\n", path, strerror(errno));
    close(pfd); return -1;
  }
  close(pfd);
  return 0;
}

int main(int argc, char **argv) {
  struct landlock_ruleset_attr rs;
  unsigned long handled;
  int rfd, abi;
  char *spec, *tok, *save = NULL;

  if (argc < 4 || strcmp(argv[2], "--") != 0) {
    fprintf(stderr, "usage: 0router-gate rw:PATH,ro:PATH... -- prog [args...]\n");
    return 126;
  }

  abi = (int)landlock_create_ruleset(NULL, 0, LANDLOCK_CREATE_RULESET_VERSION);
  if (abi < 1) { fprintf(stderr, "0router-gate: Landlock unavailable (abi=%d)\n", abi); return 126; }

  handled = LANDLOCK_ACCESS_FS_EXECUTE | LANDLOCK_ACCESS_FS_WRITE_FILE |
            LANDLOCK_ACCESS_FS_READ_FILE | LANDLOCK_ACCESS_FS_READ_DIR |
            LANDLOCK_ACCESS_FS_REMOVE_DIR | LANDLOCK_ACCESS_FS_REMOVE_FILE |
            LANDLOCK_ACCESS_FS_MAKE_CHAR | LANDLOCK_ACCESS_FS_MAKE_DIR |
            LANDLOCK_ACCESS_FS_MAKE_REG | LANDLOCK_ACCESS_FS_MAKE_SOCK |
            LANDLOCK_ACCESS_FS_MAKE_FIFO;
  if (abi >= 2) handled |= LANDLOCK_ACCESS_FS_REFER | LANDLOCK_ACCESS_FS_TRUNCATE;
#ifdef LANDLOCK_ACCESS_FS_IOCTL_DEV
  if (abi >= 5) handled |= LANDLOCK_ACCESS_FS_IOCTL_DEV;
#elif defined(LANDLOCK_ACCESS_FS_IOCTL)
  if (abi >= 5) handled |= LANDLOCK_ACCESS_FS_IOCTL;
#endif

#define RO_ACC (LANDLOCK_ACCESS_FS_EXECUTE | LANDLOCK_ACCESS_FS_READ_FILE | LANDLOCK_ACCESS_FS_READ_DIR)

  rs.handled_access_fs = handled;
  rfd = (int)landlock_create_ruleset(&rs, sizeof(rs), 0);
  if (rfd < 0) { fprintf(stderr, "0router-gate: create_ruleset: %s\n", strerror(errno)); return 126; }

  spec = argv[1];
  tok = strtok_r(spec, ",", &save);
  while (tok) {
    char *p = tok;
    unsigned long acc = RO_ACC;
    if (strncmp(p, "rw:", 3) == 0) { acc = handled; p += 3; }
    else if (strncmp(p, "ro:", 3) == 0) { p += 3; }
    else { fprintf(stderr, "0router-gate: bad rule '%s'\n", tok); return 126; }
    if (rule_add(rfd, p, acc)) return 126;
    tok = strtok_r(NULL, ",", &save);
  }

  if (prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0)) {
    fprintf(stderr, "0router-gate: prctl no_new_privs: %s\n", strerror(errno));
    return 126;
  }
  if (landlock_restrict_self(rfd, 0)) {
    fprintf(stderr, "0router-gate: restrict_self: %s\n", strerror(errno));
    return 126;
  }
  close(rfd);

  execvp(argv[3], &argv[3]);
  fprintf(stderr, "0router-gate: exec %s: %s\n", argv[3], strerror(errno));
  return 126;
}
