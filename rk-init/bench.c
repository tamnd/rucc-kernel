/*
 * rk-bench: the kernel paths of docs/plan/12-performance.md that a static program can time
 * without perf, lmbench or will-it-scale in the initramfs.
 *
 * rk bench builds this with the host gcc and puts it in the initramfs as /bench, so the user
 * code is the same for both kernels and only the kernel differs. Each benchmark runs a fixed
 * number of operations, five times, and prints the fastest of the five as one line:
 *
 *     RK-BENCH <name> <nanoseconds per operation>
 *
 * The fastest is kept because a guest under TCG on a busy host is stopped now and then, and a
 * stop only makes a pass slower. memcpy is the control. It runs no kernel code, so a difference
 * there is the host and not the kernel.
 */
#define _GNU_SOURCE
#include <fcntl.h>
#include <linux/futex.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

#define PASSES 5
#define PAGES 256

static char *self;

static double now(void)
{
	struct timespec t;

	clock_gettime(CLOCK_MONOTONIC, &t);
	return t.tv_sec * 1e9 + t.tv_nsec;
}

static void fail(const char *what)
{
	perror(what);
	exit(1);
}

static void getppid_loop(long n)
{
	for (long i = 0; i < n; i++)
		syscall(SYS_getppid);
}

static void read_loop(long n)
{
	char c;
	int fd = open("/dev/zero", O_RDONLY);

	for (long i = 0; i < n; i++)
		if (read(fd, &c, 1) != 1)
			fail("read");
	close(fd);
}

static void write_loop(long n)
{
	char c = 0;
	int fd = open("/dev/null", O_WRONLY);

	for (long i = 0; i < n; i++)
		if (write(fd, &c, 1) != 1)
			fail("write");
	close(fd);
}

static void open_loop(long n)
{
	for (long i = 0; i < n; i++) {
		int fd = open("/tmp/rk-bench", O_RDONLY);

		if (fd < 0)
			fail("open");
		close(fd);
	}
}

static void stat_loop(long n)
{
	struct stat st;

	for (long i = 0; i < n; i++)
		if (stat("/tmp/rk-bench", &st))
			fail("stat");
}

/* One byte to the child and one back, through two pipes or one socket pair. */
static void ping_pong(long n, int unix_socket)
{
	int there[2], back[2];
	char c = 0;
	pid_t child;

	if (unix_socket) {
		if (socketpair(AF_UNIX, SOCK_STREAM, 0, there))
			fail("socketpair");
		back[0] = there[0];
		back[1] = there[1];
	} else if (pipe(there) || pipe(back)) {
		fail("pipe");
	}
	child = fork();
	if (child < 0)
		fail("fork");
	if (child == 0) {
		int in = unix_socket ? there[1] : there[0];
		int out = unix_socket ? there[1] : back[1];

		for (long i = 0; i < n; i++)
			if (read(in, &c, 1) != 1 || write(out, &c, 1) != 1)
				_exit(1);
		_exit(0);
	}
	for (long i = 0; i < n; i++) {
		int out = unix_socket ? there[0] : there[1];
		int in = unix_socket ? there[0] : back[0];

		if (write(out, &c, 1) != 1 || read(in, &c, 1) != 1)
			fail("ping");
	}
	waitpid(child, NULL, 0);
	close(there[0]);
	close(there[1]);
	if (!unix_socket) {
		close(back[0]);
		close(back[1]);
	}
}

static void pipe_loop(long n)
{
	ping_pong(n, 0);
}

static void unix_loop(long n)
{
	ping_pong(n, 1);
}

static void fork_loop(long n)
{
	for (long i = 0; i < n; i++) {
		pid_t child = fork();

		if (child < 0)
			fail("fork");
		if (child == 0)
			_exit(0);
		waitpid(child, NULL, 0);
	}
}

static void exec_loop(long n)
{
	for (long i = 0; i < n; i++) {
		pid_t child = fork();

		if (child < 0)
			fail("fork");
		if (child == 0) {
			execl(self, self, "noop", (char *)NULL);
			_exit(1);
		}
		waitpid(child, NULL, 0);
	}
}

/* n is in pages, PAGES to one mapping. */
static void fault_loop(long n)
{
	long size = PAGES * 4096L;

	for (long i = 0; i < n / PAGES; i++) {
		char *p = mmap(NULL, size, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);

		if (p == MAP_FAILED)
			fail("mmap");
		for (long at = 0; at < size; at += 4096)
			p[at] = 1;
		munmap(p, size);
	}
}

static void mmap_loop(long n)
{
	for (long i = 0; i < n; i++) {
		void *p = mmap(NULL, 4096, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);

		if (p == MAP_FAILED)
			fail("mmap");
		munmap(p, 4096);
	}
}

static void brk_loop(long n)
{
	char *base = sbrk(0);

	for (long i = 0; i < n; i++) {
		if (brk(base + 4096 * 16) || brk(base))
			fail("brk");
	}
}

static volatile sig_atomic_t caught;

static void on_signal(int sig)
{
	(void)sig;
	caught++;
}

static void signal_loop(long n)
{
	struct sigaction sa;

	memset(&sa, 0, sizeof(sa));
	sa.sa_handler = on_signal;
	sigaction(SIGUSR1, &sa, NULL);
	for (long i = 0; i < n; i++)
		kill(getpid(), SIGUSR1);
}

static void futex_loop(long n)
{
	static int word;

	for (long i = 0; i < n; i++)
		syscall(SYS_futex, &word, FUTEX_WAKE_PRIVATE, 1, NULL, NULL, 0);
}

static char from[1 << 16], to[1 << 16];

static void memcpy_loop(long n)
{
	for (long i = 0; i < n; i++) {
		from[i & 0xffff] = (char)i;
		memcpy(to, from, sizeof(from));
		__asm__ volatile("" : : "r"(to) : "memory");
	}
}

struct bench {
	const char *name;
	void (*run)(long);
	long ops;
};

static const struct bench benches[] = {
	{ "getppid", getppid_loop, 200000 },
	{ "read", read_loop, 50000 },
	{ "write", write_loop, 50000 },
	{ "open-close", open_loop, 5000 },
	{ "stat", stat_loop, 5000 },
	{ "pipe-pingpong", pipe_loop, 2000 },
	{ "unix-pingpong", unix_loop, 1000 },
	{ "fork-exit", fork_loop, 200 },
	{ "fork-exec", exec_loop, 20 },
	{ "page-fault", fault_loop, PAGES * 50 },
	{ "mmap-munmap", mmap_loop, 2500 },
	{ "brk", brk_loop, 5000 },
	{ "signal", signal_loop, 10000 },
	{ "futex-wake", futex_loop, 200000 },
	{ "memcpy", memcpy_loop, 20000 },
};

/* Time one benchmark, print its line and exit. A failure exits with 1 from fail(). */
static void time_one(const struct bench *bench)
{
	double best = 0;

	bench->run(bench->ops / 10);
	for (int pass = 0; pass < PASSES; pass++) {
		double start = now();

		bench->run(bench->ops);
		double took = (now() - start) / bench->ops;
		if (pass == 0 || took < best)
			best = took;
	}
	printf("RK-BENCH %s %.1f\n", bench->name, best);
	fflush(stdout);
	exit(0);
}

int main(int argc, char **argv)
{
	int fd;

	if (argc > 1 && strcmp(argv[1], "noop") == 0)
		return 0;
	self = argv[0];
	fd = open("/tmp/rk-bench", O_CREAT | O_WRONLY, 0644);
	if (fd < 0)
		fail("/tmp/rk-bench");
	close(fd);
	for (size_t b = 0; b < sizeof(benches) / sizeof(benches[0]); b++) {
		const struct bench *bench = &benches[b];
		int status;
		pid_t child;

		/*
		 * Each benchmark runs in a child of its own, so that a kernel built without a feature
		 * (tinyconfig has no AF_UNIX) skips that one benchmark and not all the ones after it.
		 */
		child = fork();
		if (child < 0)
			fail("fork");
		if (child == 0)
			time_one(bench);
		if (waitpid(child, &status, 0) != child || !WIFEXITED(status) ||
		    WEXITSTATUS(status) != 0) {
			printf("RK-BENCH-SKIP %s\n", bench->name);
			fflush(stdout);
		}
	}
	unlink("/tmp/rk-bench");
	return 0;
}
