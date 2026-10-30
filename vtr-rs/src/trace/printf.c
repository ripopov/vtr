/* Only normalized single conversions and matching typed arguments reach libc. */
#include <stdio.h>
#include <stdint.h>
#include <stdlib.h>
#include <wchar.h>

int vtr_printf_scalar(char *out, size_t size, const char *format, int kind,
                      uint64_t bits, double number, const char *text) {
    switch (kind) {
    case 0: return snprintf(out, size, format, (long long)(int64_t)bits);
    case 1: return snprintf(out, size, format, (unsigned long long)bits);
    case 2: return snprintf(out, size, format, number);
    case 3: return snprintf(out, size, format, (long double)number);
    case 4: return snprintf(out, size, format, text);
    case 5: return snprintf(out, size, format, (int)(bits & 255));
    case 6: return snprintf(out, size, format, (wint_t)bits);
    case 7: return snprintf(out, size, format, (void *)(uintptr_t)bits);
    case 8: {
        mbstate_t state = {0};
        const char *input = text;
        size_t count = mbsrtowcs(NULL, &input, 0, &state);
        if (count == (size_t)-1 || count > 65536) return -1;
        wchar_t *wide = calloc(count + 1, sizeof(wchar_t));
        if (!wide) return -1;
        state = (mbstate_t){0};
        input = text;
        int result = -1;
        if (mbsrtowcs(wide, &input, count + 1, &state) == count)
            result = snprintf(out, size, format, wide);
        free(wide);
        return result;
    }
    default: return -1;
    }
}
