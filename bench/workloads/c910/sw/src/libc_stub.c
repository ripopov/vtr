// The few libc routines GCC may emit calls to in a -nostdlib build.
#include <stddef.h>

void *memcpy(void *d, const void *s, size_t n) {
  unsigned char *dp = d; const unsigned char *sp = s;
  while (n--) *dp++ = *sp++;
  return d;
}

void *memset(void *d, int c, size_t n) {
  unsigned char *dp = d;
  while (n--) *dp++ = (unsigned char)c;
  return d;
}

void *memmove(void *d, const void *s, size_t n) {
  unsigned char *dp = d; const unsigned char *sp = s;
  if (dp < sp) { while (n--) *dp++ = *sp++; }
  else { dp += n; sp += n; while (n--) *--dp = *--sp; }
  return d;
}

int memcmp(const void *a, const void *b, size_t n) {
  const unsigned char *x = a, *y = b;
  for (; n--; ++x, ++y) if (*x != *y) return (int)*x - (int)*y;
  return 0;
}

size_t strlen(const char *s) { const char *p = s; while (*p) ++p; return (size_t)(p - s); }
