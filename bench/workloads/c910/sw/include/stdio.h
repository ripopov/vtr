// Minimal freestanding <stdio.h> for the C910 bare-metal benchmarks. Only the
// declarations used by the T-Head test sources and the bundled tiny printf are
// provided; there is no host libc in this build.
#ifndef BENCH_STDIO_H
#define BENCH_STDIO_H

#include <stdarg.h>
#include <stddef.h>

typedef int FILE;

int printf(const char *format, ...);
int sprintf(char *buffer, const char *format, ...);
int snprintf(char *buffer, size_t count, const char *format, ...);
int vsnprintf(char *buffer, size_t count, const char *format, va_list va);
int vprintf(const char *format, va_list va);
int putchar(int c);
int puts(const char *s);
int fputc(int ch, FILE *stream);

#endif
