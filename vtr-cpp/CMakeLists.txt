cmake_minimum_required(VERSION 3.20)
project(vtr VERSION 4.0.0 LANGUAGES CXX)

include(CTest)
include(GNUInstallDirs)
option(VTR_BUILD_EXAMPLES "Build API tutorials" ON)
option(VTR_SANITIZE "Enable address and undefined behavior sanitizers" OFF)
option(VTR_THREAD_SANITIZE "Enable the thread sanitizer" OFF)
if(VTR_SANITIZE AND VTR_THREAD_SANITIZE)
    message(FATAL_ERROR "Select either address/undefined or thread sanitizers")
endif()

include(cmake/VtrDependencies.cmake)

add_library(vtr
    src/common.cpp
    src/reader.cpp
    src/writer.cpp
    src/engine/activity.cpp
    src/engine/container.cpp
    src/engine/handoff.cpp
    src/engine/reader.cpp
    src/engine/records.cpp
    src/engine/schema.cpp
    src/engine/stream.cpp
    src/engine/transactions.cpp
    src/engine/writer.cpp
    src/wire/format.cpp
    src/wire/reader.cpp
    src/wire/writer.cpp
)
add_library(vtr::vtr ALIAS vtr)
target_compile_features(vtr PUBLIC cxx_std_20)
target_include_directories(vtr
    PUBLIC $<BUILD_INTERFACE:${CMAKE_CURRENT_SOURCE_DIR}>
           $<INSTALL_INTERFACE:${CMAKE_INSTALL_INCLUDEDIR}/vtr>
    PRIVATE src/include src/engine src/wire
)
target_include_directories(vtr SYSTEM PRIVATE
    third_party/unordered_dense/include)
target_link_libraries(vtr PRIVATE ZLIB::ZLIB Threads::Threads
    Vtr::LZ4 Vtr::Zstd)
if(CMAKE_CXX_COMPILER_ID MATCHES "GNU|Clang")
    target_compile_options(vtr PRIVATE -Wall -Wextra -Wpedantic -Werror)
endif()
if(VTR_SANITIZE)
    target_compile_options(vtr PUBLIC -fsanitize=address,undefined -fno-omit-frame-pointer)
    target_link_options(vtr PUBLIC -fsanitize=address,undefined)
endif()
if(VTR_THREAD_SANITIZE)
    target_compile_options(vtr PUBLIC -fsanitize=thread -fno-omit-frame-pointer)
    target_link_options(vtr PUBLIC -fsanitize=thread)
endif()

if(BUILD_TESTING)
    add_executable(vtr_api_test tests/api_test.cpp)
    target_link_libraries(vtr_api_test PRIVATE vtr)
    add_test(NAME typed_public_api COMMAND vtr_api_test ${CMAKE_CURRENT_BINARY_DIR}/api-fixtures)
    set_tests_properties(typed_public_api PROPERTIES TIMEOUT 120)
    add_executable(vtr_tracker_test tests/tracker_test.cpp)
    target_link_libraries(vtr_tracker_test PRIVATE vtr)
    add_test(NAME keyed_transaction_lifetimes COMMAND vtr_tracker_test
        ${CMAKE_CURRENT_BINARY_DIR}/tracker-fixtures)
    set_tests_properties(keyed_transaction_lifetimes PROPERTIES TIMEOUT 120)
    add_executable(vtr_declarations_test tests/declarations_test.cpp)
    target_link_libraries(vtr_declarations_test PRIVATE vtr)
    add_test(NAME hdl_declarations COMMAND vtr_declarations_test
        ${CMAKE_CURRENT_BINARY_DIR}/declaration-fixtures)
    add_executable(vtr_logging_example_test tests/logging_example_test.cpp)
    target_link_libraries(vtr_logging_example_test PRIVATE vtr)
    add_test(NAME logging_callsite_capture COMMAND vtr_logging_example_test
        ${CMAKE_CURRENT_BINARY_DIR}/logging-fixtures)
    add_executable(vtr_dpi_test tests/dpi_test.cpp)
    target_include_directories(vtr_dpi_test PRIVATE tests/dpi_stub)
    target_link_libraries(vtr_dpi_test PRIVATE vtr)
    add_test(NAME serialized_simulator_dpi COMMAND vtr_dpi_test
        ${CMAKE_CURRENT_BINARY_DIR}/dpi-fixtures)
    find_package(Python3 COMPONENTS Interpreter REQUIRED)
    function(vtr_engine_test name source)
        add_executable(${name} tests/engine/${source}.cpp)
        target_link_libraries(${name} PRIVATE vtr)
        target_include_directories(${name} PRIVATE src/include src/engine src/wire)
        target_include_directories(${name} SYSTEM PRIVATE third_party/unordered_dense/include)
        add_test(NAME ${name} COMMAND ${name} ${CMAKE_CURRENT_BINARY_DIR}/${name}-fixtures)
        set_tests_properties(${name} PROPERTIES TIMEOUT 180)
    endfunction()
    vtr_engine_test(schema_payloads schema_test)
    vtr_engine_test(record_pages records_test)
    vtr_engine_test(container_recovery container_test)
    vtr_engine_test(transaction_pages transactions_test)
    vtr_engine_test(writer_reader_lifecycle lifecycle_test)
    vtr_engine_test(full_file_invariants full_validation_test)
    vtr_engine_test(activity_identity_and_windows activity_test)
    vtr_engine_test(writer_failure_atomicity failure_test)
    vtr_engine_test(bounded_ordered_handoff handoff_test)
    vtr_engine_test(owned_worker_cpu thread_cpu_test)
    vtr_engine_test(waveform_workers wave_worker_test)
    vtr_engine_test(concurrent_writer_snapshots async_writer_test)
    vtr_engine_test(batched_capture_and_owned_payload batch_test)
    vtr_engine_test(reusable_capture_budget_and_lifetime payload_pool_test)
    vtr_engine_test(spsc_publication_and_wraparound spsc_test)
    vtr_engine_test(atomic_record_id_exhaustion record_ids_test)
    vtr_engine_test(producer_assigned_id_lookup assigned_ids_test)
    vtr_engine_test(spsc_time_ordering_and_progress stream_test)
    add_executable(scalar_page_invariants tests/engine/scalar_page_test.cpp)
    target_link_libraries(scalar_page_invariants PRIVATE vtr)
    target_include_directories(scalar_page_invariants PRIVATE src/include src/engine src/wire)
    target_include_directories(scalar_page_invariants SYSTEM PRIVATE third_party/unordered_dense/include)
    add_test(NAME scalar_page_invariants COMMAND scalar_page_invariants)
    foreach(pair IN ITEMS "schema;schema_payloads" "records;record_pages"
                          "transactions;transaction_pages" "scalar;scalar_page_invariants")
        list(GET pair 0 oracle)
        list(GET pair 1 executable)
        add_test(NAME independent_${oracle}_oracle COMMAND ${Python3_EXECUTABLE}
            ${CMAKE_CURRENT_SOURCE_DIR}/tests/engine/${oracle}_oracle.py
            $<TARGET_FILE:${executable}>)
    endforeach()
    add_test(NAME independent_activity_oracle COMMAND ${Python3_EXECUTABLE}
        ${CMAKE_CURRENT_SOURCE_DIR}/tests/engine/activity_oracle.py
        $<TARGET_FILE:activity_identity_and_windows>
        ${CMAKE_CURRENT_BINARY_DIR}/activity-oracle-fixtures)
    add_executable(log_format_oracle tests/log_format_oracle.cpp)
    target_compile_features(log_format_oracle PRIVATE cxx_std_20)
    if(EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/../vtr/Cargo.toml")
        add_test(NAME scalar_format_reference COMMAND log_format_oracle --check
            ${CMAKE_CURRENT_SOURCE_DIR}/../vtr/tests/fixtures/log-format.json)
        set_tests_properties(scalar_format_reference PROPERTIES TIMEOUT 30)
        add_executable(vtr_typed_interop tests/typed_interop.cpp)
        target_link_libraries(vtr_typed_interop PRIVATE vtr)
        add_test(NAME rust_typed_interoperability COMMAND ${Python3_EXECUTABLE}
            ${CMAKE_CURRENT_SOURCE_DIR}/tests/typed_interop.py $<TARGET_FILE:vtr_typed_interop>
            ${CMAKE_CURRENT_BINARY_DIR}/typed-interop $<TARGET_FILE:vtr_tracker_test>)
        set_tests_properties(rust_typed_interoperability PROPERTIES TIMEOUT 180)
    endif()
endif()

if(VTR_BUILD_EXAMPLES)
    foreach(example IN ITEMS write_transactions read_transactions waveforms parallel_recording
                             relationships logging recovery)
        add_executable(vtr_${example} examples/${example}.cpp)
        target_link_libraries(vtr_${example} PRIVATE vtr)
        if(BUILD_TESTING)
            if(example STREQUAL "read_transactions")
                add_test(NAME tutorial_${example} COMMAND vtr_${example} tutorial.vtr)
                set_tests_properties(tutorial_${example} PROPERTIES DEPENDS tutorial_write_transactions)
            elseif(example STREQUAL "write_transactions")
                add_test(NAME tutorial_${example} COMMAND vtr_${example} tutorial.vtr)
            else()
                add_test(NAME tutorial_${example} COMMAND vtr_${example})
            endif()
        endif()
    endforeach()
endif()

include(CMakePackageConfigHelpers)
configure_package_config_file(cmake/vtrConfig.cmake.in
    ${CMAKE_CURRENT_BINARY_DIR}/vtrConfig.cmake
    INSTALL_DESTINATION ${CMAKE_INSTALL_LIBDIR}/cmake/vtr)
write_basic_package_version_file(${CMAKE_CURRENT_BINARY_DIR}/vtrConfigVersion.cmake
    VERSION ${PROJECT_VERSION} COMPATIBILITY SameMajorVersion)
install(TARGETS vtr EXPORT vtrTargets
    ARCHIVE DESTINATION ${CMAKE_INSTALL_LIBDIR}
    LIBRARY DESTINATION ${CMAKE_INSTALL_LIBDIR}
    RUNTIME DESTINATION ${CMAKE_INSTALL_BINDIR})
install(FILES vtr_common.h vtr_reader.h vtr_writer.h vtr_track.h vtr_trace_dpi.h vtr_trace.sv
    DESTINATION ${CMAKE_INSTALL_INCLUDEDIR}/vtr)
install(EXPORT vtrTargets NAMESPACE vtr:: DESTINATION ${CMAKE_INSTALL_LIBDIR}/cmake/vtr)
install(FILES ${CMAKE_CURRENT_BINARY_DIR}/vtrConfig.cmake
              ${CMAKE_CURRENT_BINARY_DIR}/vtrConfigVersion.cmake
              cmake/VtrDependencies.cmake
    DESTINATION ${CMAKE_INSTALL_LIBDIR}/cmake/vtr)
install(FILES LICENSE DESTINATION ${CMAKE_INSTALL_DATADIR}/licenses/vtr)
install(FILES third_party/unordered_dense/LICENSE third_party/unordered_dense/UPSTREAM.json
    DESTINATION ${CMAKE_INSTALL_DATADIR}/licenses/vtr/unordered_dense)
