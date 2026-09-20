/*
 * Compatibility bridge for the PRS-T1's Android 2.2 Wi-Fi library.
 *
 * The agent is a static musl executable and must not link to Android Bionic.
 * This small ET_EXEC ARM binary runs with the T1 linker and resolves the
 * legacy Wi-Fi entry points from libhardware_legacy.so at runtime.
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

typedef int (*wifi_function)(void);

static int selected_function(const char *action, const char **symbol)
{
    if (string_equal(action, "load-driver")) {
        *symbol = "wifi_load_driver";
        return 0;
    }
    if (string_equal(action, "unload-driver")) {
        *symbol = "wifi_unload_driver";
        return 1;
    }
    if (string_equal(action, "start-supplicant")) {
        *symbol = "wifi_start_supplicant";
        return 2;
    }
    if (string_equal(action, "stop-supplicant")) {
        *symbol = "wifi_stop_supplicant";
        return 3;
    }
    return -1;
}

void wifi_main(int argc, const char **argv)
{
    const char *symbol;
    int function_number;

    if (argc != 2) {
        exit_now(64);
    }
    function_number = selected_function(argv[1], &symbol);
    if (function_number < 0) {
        exit_now(64);
    }

    void *library = dlopen("/system/lib/libhardware_legacy.so", 2);
    if (!library) {
        exit_now(111);
    }

    wifi_function function = (wifi_function)dlsym(library, symbol);
    if (!function) {
        exit_now(112 + function_number);
    }

    if (function() != 0) {
        exit_now(120 + function_number);
    }
    exit_now(0);
}

/* Linux enters an ARM process with argc at [sp] and argv at [sp + 4]. */
__attribute__((naked, noreturn)) void _start(void)
{
    __asm__ volatile(
        "ldr r0, [sp]\n"
        "add r1, sp, #4\n"
        "b wifi_main\n");
}
