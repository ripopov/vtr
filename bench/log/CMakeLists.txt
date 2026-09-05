cmake_minimum_required(VERSION 3.16)
project(vtr_log_bench C CXX)
set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
if(NOT CMAKE_BUILD_TYPE)
  set(CMAKE_BUILD_TYPE Release)
endif()
set(ROOT ${CMAKE_CURRENT_SOURCE_DIR}/../..)
set(EXT ${ROOT}/ext)
set(VTR_TARGET ${ROOT}/target/release)
set(OPT -O3 -DNDEBUG)

foreach(dep clp NanoLog binlog quill)
  if(NOT EXISTS ${EXT}/${dep})
    message(FATAL_ERROR "${EXT}/${dep} is missing: run `sh bench/log/fetch_refs.sh` first")
  endif()
endforeach()

find_path(ZSTD_INCLUDE_DIR zstd.h HINTS /opt/homebrew/include /usr/local/include)
find_library(ZSTD_LIB zstd HINTS /opt/homebrew/lib /usr/local/lib REQUIRED)
find_package(Threads REQUIRED)

# ---- text baseline and zstd tool ----
add_executable(log_text log_text.cpp)
target_compile_options(log_text PRIVATE ${OPT})

add_executable(log_zstd log_zstd.cpp)
target_include_directories(log_zstd PRIVATE ${ZSTD_INCLUDE_DIR})
target_link_libraries(log_zstd PRIVATE ${ZSTD_LIB})
target_compile_options(log_zstd PRIVATE ${OPT})

# ---- VTR (C API + vtr_log.hpp) ----
add_executable(log_vtr log_vtr.cpp)
target_include_directories(log_vtr PRIVATE ${ROOT}/crates/vtr-capi/include)
target_link_libraries(log_vtr PRIVATE ${VTR_TARGET}/libvtr.a Threads::Threads ${CMAKE_DL_LIBS} m)
target_compile_options(log_vtr PRIVATE ${OPT})

# ---- Quill (header-only) ----
add_executable(log_quill log_quill.cpp)
target_include_directories(log_quill SYSTEM PRIVATE ${EXT}/quill/include)
target_link_libraries(log_quill PRIVATE Threads::Threads)
target_compile_options(log_quill PRIVATE ${OPT})

# ---- binlog (header-only producer; bread needs the small support library) ----
add_executable(log_binlog log_binlog.cpp)
target_include_directories(log_binlog SYSTEM PRIVATE ${EXT}/binlog/include)
target_link_libraries(log_binlog PRIVATE Threads::Threads)
target_compile_options(log_binlog PRIVATE ${OPT})

add_library(binlog_support STATIC
  ${EXT}/binlog/include/binlog/EventStream.cpp
  ${EXT}/binlog/include/binlog/Time.cpp
  ${EXT}/binlog/include/binlog/ToStringVisitor.cpp
  ${EXT}/binlog/include/binlog/PrettyPrinter.cpp
  ${EXT}/binlog/include/binlog/EntryStream.cpp
  ${EXT}/binlog/include/binlog/TextOutputStream.cpp
  ${EXT}/binlog/include/binlog/detail/OstreamBuffer.cpp)
target_include_directories(binlog_support SYSTEM PUBLIC ${EXT}/binlog/include)
target_compile_options(binlog_support PRIVATE ${OPT} -w)
add_executable(bread ${EXT}/binlog/bin/bread.cpp ${EXT}/binlog/bin/printers.cpp)
target_link_libraries(bread PRIVATE binlog_support)
target_compile_options(bread PRIVATE ${OPT} -w)

# ---- NanoLog (runtime ported to this host by nanolog_port.py) ----
set(NANOLOG_PORT ${CMAKE_CURRENT_BINARY_DIR}/nanolog)
if(NOT EXISTS ${NANOLOG_PORT}/runtime/GeneratedCode.cc)
  execute_process(COMMAND ${Python3_EXECUTABLE} python3 ${CMAKE_CURRENT_SOURCE_DIR}/nanolog_port.py ${EXT}/NanoLog ${NANOLOG_PORT}
                  RESULT_VARIABLE port_rc)
  if(NOT port_rc EQUAL 0)
    message(FATAL_ERROR "NanoLog port failed")
  endif()
endif()
set(NL ${NANOLOG_PORT}/runtime)
add_library(nanolog STATIC ${NL}/Cycles.cc ${NL}/Util.cc ${NL}/Log.cc ${NL}/NanoLog.cc ${NL}/RuntimeLogger.cc ${NL}/GeneratedCode.cc)
target_include_directories(nanolog SYSTEM PUBLIC ${NL})
target_compile_options(nanolog PRIVATE -std=c++17 ${OPT} -w)
target_link_libraries(nanolog PUBLIC Threads::Threads)
add_executable(log_nanolog log_nanolog.cpp)
target_link_libraries(log_nanolog PRIVATE nanolog)
target_compile_options(log_nanolog PRIVATE ${OPT} -Wno-format)
add_executable(nanolog_decompressor ${NL}/LogDecompressor.cc)
target_link_libraries(nanolog_decompressor PRIVATE nanolog)
target_compile_options(nanolog_decompressor PRIVATE -std=c++17 ${OPT} -w)

# ---- CLP IR (parser and encoders compiled from ext/clp) ----
set(CLP_SRC ${EXT}/clp/components/core/src)
add_library(clp_ir STATIC
  ${CLP_SRC}/clp/ir/parsing.cpp
  ${CLP_SRC}/clp/string_utils/string_utils.cpp
  ${CLP_SRC}/clp/ffi/encoding_methods.cpp)
target_include_directories(clp_ir SYSTEM PUBLIC ${CLP_SRC} ${CLP_SRC}/clp)
target_compile_options(clp_ir PRIVATE ${OPT} -w)
add_executable(log_clp log_clp.cpp)
target_include_directories(log_clp PRIVATE ${ZSTD_INCLUDE_DIR})
target_link_libraries(log_clp PRIVATE clp_ir ${ZSTD_LIB})
target_compile_options(log_clp PRIVATE ${OPT})
