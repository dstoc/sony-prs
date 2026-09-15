/*
 * Compatibility bridge for the PRS-T1's Android 2.2 power library.
 *
 * This is deliberately a tiny old-style dynamically linked executable. The
 * T1's /system/bin/linker predates PIE support, so it must be linked as an
 * ET_EXEC ARM binary and run with the device's linker. No Android headers or
 * libc startup code are required: the entry point reads argc/argv directly
 * from the Linux initial stack, then resolves Sony's exported
 * set_screen_state() through libhardware_legacy.so.
 */

extern void *dlopen(const char *name, int flags);
extern void *dlsym(void *handle, const char *name);

static __attribute__((noreturn)) void exit_now(int code)
{
    register int r0 __asm__("r0") = code;
    register int r7 __asm__("r7") = 1;

    __asm__ volatile("svc 0" : : "r"(r0), "r"(r7) : "memory");
    for (;;) {}
}

static int string_equal(const char *left, const char *right)
{
    while (*left && *left == *right) {
        ++left;
        ++right;
    }
    return *left == *right;
}

void power_state_main(int argc, const char **argv)
{
    int state = 2; /* standby: the safest useful default for manual use */

    if (argc > 1) {
        if (string_equal(argv[1], "on")) {
            state = 1;
        } else if (string_equal(argv[1], "mem")) {
            state = 0;
        } else if (string_equal(argv[1], "standby")) {
            state = 2;
        } else {
            exit_now(64);
        }
    }

    void *library = dlopen("/system/lib/libhardware_legacy.so", 2);
    if (!library) {
        exit_now(111);
    }

    void (*set_screen_state)(int) =
        (void (*)(int))dlsym(library, "set_screen_state");
    if (!set_screen_state) {
        exit_now(112);
    }

    set_screen_state(state);
    exit_now(0);
}

/* Linux enters an ARM process with argc at [sp] and argv at [sp + 4]. */
__attribute__((naked, noreturn)) void _start(void)
{
    __asm__ volatile(
        "ldr r0, [sp]\n"
        "add r1, sp, #4\n"
        "b power_state_main\n");
}
