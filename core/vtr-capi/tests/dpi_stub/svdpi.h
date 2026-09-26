/* Minimal svdpi.h for testing vtr_trace_dpi.hpp outside a simulator: the
 * calling instance of a context import is whatever the test last set. */
#ifndef VTR_TEST_SVDPI_H
#define VTR_TEST_SVDPI_H
typedef void* svScope;
extern const char* g_test_scope;
inline svScope svGetScope() { return nullptr; }
inline const char* svGetNameFromScope(svScope) { return g_test_scope; }
#endif
