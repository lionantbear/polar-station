cmake_minimum_required(VERSION 3.18)
project(polar_cas_fs_native LANGUAGES C)
option(POLAR_EROFS "Build the pinned EROFS encoder/reader" ON)
include(CTest)
if(NOT CMAKE_SYSTEM_NAME STREQUAL "Linux")
  message(FATAL_ERROR "PolarCAS native adapter currently requires Linux")
endif()
set(CMAKE_POSITION_INDEPENDENT_CODE ON)
set(CMAKE_ARCHIVE_OUTPUT_DIRECTORY "${CMAKE_BINARY_DIR}/lib")
set(CMAKE_C_STANDARD 11)
set(CMAKE_C_EXTENSIONS ON)
find_package(Threads REQUIRED)

add_library(polar_cas_fs_io_v1 STATIC
  pcfs_io.c pcfs_pack_plan.c pcfs_tree_observer.c pcfs_generated_seal.c
  pcfs_verity.c pcfs_mount.c pcfs_overlay.c)
target_include_directories(polar_cas_fs_io_v1 PUBLIC "${CMAKE_CURRENT_SOURCE_DIR}")
target_compile_options(polar_cas_fs_io_v1 PRIVATE -Wall -Wextra)
target_link_libraries(polar_cas_fs_io_v1 PUBLIC Threads::Threads)

if(POLAR_EROFS)
  set(original "${CMAKE_CURRENT_SOURCE_DIR}/vendor/erofs-utils")
  # Package-local source is checked before executing its build machinery.
  file(STRINGS "${CMAKE_CURRENT_SOURCE_DIR}/upstream.sha256" expected_lines)
  set(expected_files "")
  foreach(record IN LISTS expected_lines)
    string(SUBSTRING "${record}" 0 64 expected)
    string(SUBSTRING "${record}" 66 -1 relative)
    string(REGEX REPLACE "^\\./" "" relative "${relative}")
    if(relative MATCHES "(^/|(^|/)\\.\\.(/|$))" OR IS_SYMLINK "${original}/${relative}")
      message(FATAL_ERROR "Invalid source path in manifest: ${relative}")
    endif()
    file(SHA256 "${original}/${relative}" actual)
    if(NOT actual STREQUAL expected)
      message(FATAL_ERROR "Pinned erofs source changed: ${relative}")
    endif()
    list(APPEND expected_files "${relative}")
  endforeach()
  file(GLOB_RECURSE actual_files LIST_DIRECTORIES false RELATIVE "${original}" "${original}/*")
  list(SORT actual_files)
  list(SORT expected_files)
  if(NOT "${actual_files}" STREQUAL "${expected_files}")
    message(FATAL_ERROR "Bundled EROFS source file set differs from upstream.sha256")
  endif()

  find_program(PATCH_EXECUTABLE patch REQUIRED)
  find_program(AUTORECONF_EXECUTABLE autoreconf REQUIRED)
  find_program(MAKE_EXECUTABLE make REQUIRED)
  find_library(LZ4_LIBRARY NAMES lz4 REQUIRED)
  set(private "${CMAKE_BINARY_DIR}/erofs/source")
  set(objects "${CMAKE_BINARY_DIR}/erofs/build")
  # Reconfigure is deterministic; the original package tree is never patched.
  file(REMOVE_RECURSE "${private}" "${objects}")
  file(MAKE_DIRECTORY "${private}" "${objects}")
  file(COPY "${original}/" DESTINATION "${private}")
  execute_process(COMMAND "${PATCH_EXECUTABLE}" -p1 --batch --forward
    -i "${CMAKE_CURRENT_SOURCE_DIR}/patches/0001-polar-vfile.patch"
    WORKING_DIRECTORY "${private}" RESULT_VARIABLE rc OUTPUT_VARIABLE patch_output ERROR_VARIABLE patch_error)
  if(NOT rc EQUAL 0)
    message(FATAL_ERROR "Pinned patch failed: ${patch_output}\n${patch_error}")
  endif()
  file(COPY "${CMAKE_CURRENT_SOURCE_DIR}/upstream/pcfs_importer.inc" DESTINATION "${private}/lib")
  execute_process(COMMAND "${AUTORECONF_EXECUTABLE}" -fi WORKING_DIRECTORY "${private}"
    RESULT_VARIABLE rc OUTPUT_FILE "${CMAKE_BINARY_DIR}/autoreconf.log" ERROR_FILE "${CMAKE_BINARY_DIR}/autoreconf.err")
  if(NOT rc EQUAL 0)
    message(FATAL_ERROR "autoreconf failed; see ${CMAKE_BINARY_DIR}/autoreconf.err")
  endif()
  set(erofs_flags "${CMAKE_C_FLAGS} -O2 -fPIC -ffunction-sections -fdata-sections")
  execute_process(COMMAND "${CMAKE_COMMAND}" -E env
    "CC=${CMAKE_C_COMPILER}" "AR=${CMAKE_AR}" "CFLAGS=${erofs_flags}"
    "${private}/configure" --disable-shared --enable-static --disable-multithreading
    --enable-lz4 --disable-lzma --without-libzstd --without-zlib --without-libdeflate
    --without-uuid --without-xxhash --without-openssl --without-libcurl
    --without-libxml2 --without-json-c --without-libnl3 --disable-s3 --disable-oci
    --disable-fuse --disable-fanotify --disable-ublk
    WORKING_DIRECTORY "${objects}" RESULT_VARIABLE rc
    OUTPUT_FILE "${CMAKE_BINARY_DIR}/configure.log" ERROR_FILE "${CMAKE_BINARY_DIR}/configure.err")
  if(NOT rc EQUAL 0)
    message(FATAL_ERROR "EROFS configure failed; see ${CMAKE_BINARY_DIR}/configure.log and .err")
  endif()
  add_custom_command(OUTPUT "${CMAKE_BINARY_DIR}/lib/liberofs.a"
    COMMAND "${MAKE_EXECUTABLE}" -C "${objects}/lib" -j2 liberofs.la
    COMMAND "${CMAKE_COMMAND}" -E make_directory "${CMAKE_BINARY_DIR}/lib"
    COMMAND "${CMAKE_COMMAND}" -E copy_if_different "${objects}/lib/.libs/liberofs.a" "${CMAKE_BINARY_DIR}/lib/liberofs.a"
    DEPENDS "${objects}/Makefile" "${CMAKE_CURRENT_SOURCE_DIR}/patches/0001-polar-vfile.patch"
    VERBATIM)
  add_custom_target(polar_erofs_archive DEPENDS "${CMAKE_BINARY_DIR}/lib/liberofs.a")
  add_library(erofs STATIC IMPORTED GLOBAL)
  set_target_properties(erofs PROPERTIES IMPORTED_LOCATION "${CMAKE_BINARY_DIR}/lib/liberofs.a")
  add_dependencies(erofs polar_erofs_archive)
  add_dependencies(polar_cas_fs_io_v1 polar_erofs_archive)
  target_sources(polar_cas_fs_io_v1 PRIVATE pcfs_erofs_job.c pcfs_erofs_reader.c
    pcfs_erofs_build.c pcfs_filesystem_plan.c pcfs_verified_sources.c pcfs_meta_plan.c pcfs_native_meta.c pcfs_coverage.c)
  target_compile_definitions(polar_cas_fs_io_v1 PRIVATE HAVE_CONFIG_H)
  target_include_directories(polar_cas_fs_io_v1 PRIVATE "${private}/include" "${private}/lib" "${objects}")
  target_link_libraries(polar_cas_fs_io_v1 PUBLIC erofs "${LZ4_LIBRARY}")
endif()

if(BUILD_TESTING)
  add_executable(pcfs_io_test ../tests/native_io.c)
  target_link_libraries(pcfs_io_test PRIVATE polar_cas_fs_io_v1)
  target_compile_options(pcfs_io_test PRIVATE -UNDEBUG)
  add_test(NAME io COMMAND pcfs_io_test)
  add_executable(pcfs_locked_tree ../tests/locked_tree.c)
  target_link_libraries(pcfs_locked_tree PRIVATE polar_cas_fs_io_v1)
  target_compile_options(pcfs_locked_tree PRIVATE -UNDEBUG -Wall -Wextra)
  add_test(NAME locked_tree_negative COMMAND pcfs_locked_tree)
  add_executable(pcfs_locked_tree_evidence ../tests/locked_tree_evidence.c)
  target_link_libraries(pcfs_locked_tree_evidence PRIVATE polar_cas_fs_io_v1)
  target_compile_options(pcfs_locked_tree_evidence PRIVATE -UNDEBUG -Wall -Wextra -Werror)
  target_link_options(pcfs_locked_tree_evidence PRIVATE -Wl,--wrap=ioctl)
  add_test(NAME locked_tree_evidence_faults COMMAND pcfs_locked_tree_evidence)

  if(POLAR_EROFS)
    add_executable(pcfs_roundtrip ../tests/erofs_roundtrip.c)
    target_link_libraries(pcfs_roundtrip PRIVATE polar_cas_fs_io_v1)
    target_compile_options(pcfs_roundtrip PRIVATE -UNDEBUG)
    target_compile_definitions(pcfs_roundtrip PRIVATE HAVE_CONFIG_H)
    target_include_directories(pcfs_roundtrip PRIVATE "${private}/include" "${private}/lib" "${objects}")
    add_test(NAME roundtrip COMMAND pcfs_roundtrip)
    add_executable(pcfs_filesystem_roundtrip ../tests/filesystem_roundtrip.c)
    target_link_libraries(pcfs_filesystem_roundtrip PRIVATE polar_cas_fs_io_v1)
    target_compile_options(pcfs_filesystem_roundtrip PRIVATE -UNDEBUG)
    target_compile_definitions(pcfs_filesystem_roundtrip PRIVATE HAVE_CONFIG_H)
    target_include_directories(pcfs_filesystem_roundtrip PRIVATE "${private}/include" "${private}/lib" "${objects}")
    add_test(NAME filesystem_roundtrip COMMAND pcfs_filesystem_roundtrip)

  endif()
endif()

if(BUILD_TESTING)
# Explicit real-kernel gate needs a fresh caller-supplied test path. It is not an
# ignored/default ctest: unavailable fs-verity returns 77, which the gate rejects.
add_executable(pcfs_locked_tree_kernel ../tests/locked_tree_kernel.c)
target_link_libraries(pcfs_locked_tree_kernel PRIVATE polar_cas_fs_io_v1)
target_compile_options(pcfs_locked_tree_kernel PRIVATE -Wall -Wextra -Werror)
endif()
