// Caller scope supplied by the test host.

#ifndef VTR_TEST_SVDPI_H
#define VTR_TEST_SVDPI_H
typedef void* svScope;
extern const char* g_test_scope;

inline svScope svGetScope() {
    return nullptr;
}

inline const char* svGetNameFromScope(svScope) {
    return g_test_scope;
}
#endif
