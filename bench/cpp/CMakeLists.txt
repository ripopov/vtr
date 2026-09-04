cmake_minimum_required(VERSION 3.16)
project(vtr_bench_harness C CXX)
set(CMAKE_C_STANDARD 99)
set(CMAKE_CXX_STANDARD 17)
if(NOT CMAKE_BUILD_TYPE)
  set(CMAKE_BUILD_TYPE Release)
endif()
set(EXT ${CMAKE_CURRENT_SOURCE_DIR}/../../ext)
set(GTKWAVE ${EXT}/libfstwriter/integration_test/verilator_share/gtkwave)
set(VTR_TARGET ${CMAKE_CURRENT_SOURCE_DIR}/../../target/release)

find_package(ZLIB REQUIRED)
find_library(LZ4_LIB lz4 REQUIRED)

# Original GTKWave fstapi.c (writer + reader), LZ4 pack support, no fastlz.
add_library(fstapi STATIC ${GTKWAVE}/fstapi.c)
target_include_directories(fstapi PUBLIC ${GTKWAVE})
target_compile_definitions(fstapi PRIVATE HAVE_FSEEKO=1 HAVE_ALLOCA_H=1 _LARGEFILE_SOURCE _FILE_OFFSET_BITS=64)
target_compile_options(fstapi PRIVATE -O2 -w)
target_link_libraries(fstapi PUBLIC ZLIB::ZLIB ${LZ4_LIB})

# libfstwriter C++ writer.
add_library(fstcpp STATIC ${EXT}/libfstwriter/fstcpp/fstcpp_writer.cpp ${EXT}/libfstwriter/fstcpp/fstcpp_variable_info.cpp)
target_include_directories(fstcpp PUBLIC ${EXT}/libfstwriter)
target_compile_options(fstcpp PRIVATE -O2 -w)
target_link_libraries(fstcpp PUBLIC ZLIB::ZLIB ${LZ4_LIB})

add_executable(fst_write fst_write.cpp)
target_link_libraries(fst_write PRIVATE fstapi fstcpp)
target_compile_options(fst_write PRIVATE -O2)

add_executable(fst_read fst_read.c)
target_link_libraries(fst_read PRIVATE fstapi)
target_compile_options(fst_read PRIVATE -O2)

add_executable(vtr_write vtr_write.c)
target_include_directories(vtr_write PRIVATE ${CMAKE_CURRENT_SOURCE_DIR}/../../crates/vtr-capi/include)
target_link_libraries(vtr_write PRIVATE ${VTR_TARGET}/libvtr.a pthread dl m)
target_compile_options(vtr_write PRIVATE -O2)

add_executable(ftr_write ftr_write.cpp)
target_include_directories(ftr_write PRIVATE ${EXT}/LWTR4SC/src)
target_link_libraries(ftr_write PRIVATE ${LZ4_LIB})
target_compile_options(ftr_write PRIVATE -O2)

add_executable(konata_ftr konata_ftr.cpp)
target_include_directories(konata_ftr PRIVATE ${EXT}/LWTR4SC/src)
target_link_libraries(konata_ftr PRIVATE ${LZ4_LIB})
target_compile_options(konata_ftr PRIVATE -O2)
