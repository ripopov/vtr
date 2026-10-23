find_package(ZLIB REQUIRED)
find_package(Threads REQUIRED)
find_path(VTR_LZ4_INCLUDE_DIR lz4.h REQUIRED)
find_library(VTR_LZ4_LIBRARY NAMES lz4 REQUIRED)
find_path(VTR_ZSTD_INCLUDE_DIR zstd.h REQUIRED)
find_library(VTR_ZSTD_LIBRARY NAMES zstd REQUIRED)

if(NOT TARGET Vtr::LZ4)
    add_library(Vtr::LZ4 UNKNOWN IMPORTED)
    set_target_properties(Vtr::LZ4 PROPERTIES
        IMPORTED_LOCATION "${VTR_LZ4_LIBRARY}"
        INTERFACE_INCLUDE_DIRECTORIES "${VTR_LZ4_INCLUDE_DIR}")
endif()
if(NOT TARGET Vtr::Zstd)
    add_library(Vtr::Zstd UNKNOWN IMPORTED)
    set_target_properties(Vtr::Zstd PROPERTIES
        IMPORTED_LOCATION "${VTR_ZSTD_LIBRARY}"
        INTERFACE_INCLUDE_DIRECTORIES "${VTR_ZSTD_INCLUDE_DIR}")
endif()
