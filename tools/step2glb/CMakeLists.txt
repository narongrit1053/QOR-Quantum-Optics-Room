# step2glb: STEP -> GLB sidecar for LabSetup (see README.md and docs/DESIGN.md §5).
cmake_minimum_required(VERSION 3.21)
project(step2glb VERSION 0.1.0 LANGUAGES CXX)

if(NOT MSVC)
  message(FATAL_ERROR "step2glb is built with MSVC x64 against the official OpenCascade vc14 prebuilt.")
endif()

# Where fetch-occt.ps1 unpacked OpenCascade and its runtime DLLs.
set(OCCT_ROOT "D:/dev/occt-7.9.3" CACHE PATH "Folder created by fetch-occt.ps1")

# The prebuilt ships Release binaries only (/MD). A Debug build would mix CRTs.
if(NOT CMAKE_CONFIGURATION_TYPES AND NOT CMAKE_BUILD_TYPE)
  set(CMAKE_BUILD_TYPE Release CACHE STRING "" FORCE)
endif()
set(CMAKE_MAP_IMPORTED_CONFIG_RELWITHDEBINFO Release)
set(CMAKE_MAP_IMPORTED_CONFIG_MINSIZEREL Release)
set(CMAKE_MAP_IMPORTED_CONFIG_DEBUG Release)

# OpenCASCADEConfigVersion.cmake only accepts an exact version.
find_package(OpenCASCADE 7.9.3 EXACT REQUIRED CONFIG
  COMPONENTS FoundationClasses ModelingData ModelingAlgorithms Visualization ApplicationFramework DataExchange
  HINTS "${OCCT_ROOT}/opencascade-7.9.3-vc14-64")

add_executable(step2glb main.cpp)
target_compile_features(step2glb PRIVATE cxx_std_17)
# /EHa lets OSD::SetSignal turn access violations inside OpenCascade into C++ exceptions.
target_compile_options(step2glb PRIVATE /EHa /W4 /utf-8 /permissive-)
target_compile_definitions(step2glb PRIVATE NOMINMAX UNICODE _UNICODE)
target_link_libraries(step2glb PRIVATE
  TKernel TKMath TKG3d TKBRep TKTopAlgo TKMesh
  TKLCAF TKCAF TKXCAF TKXSBase TKDE TKDESTEP TKRWMesh TKDEGLTF)

# Make the build folder self-contained: OpenCascade toolkits the exe needs (transitively,
# via the imported targets) plus the third-party DLLs those toolkits link.
file(GLOB_RECURSE OCCT_THIRDPARTY_DLLS "${OCCT_ROOT}/3rdparty-vc14-64/*/bin/*.dll")
list(FILTER OCCT_THIRDPARTY_DLLS EXCLUDE REGEX "/debug/")
if(NOT OCCT_THIRDPARTY_DLLS)
  message(FATAL_ERROR "No third-party DLLs under ${OCCT_ROOT}/3rdparty-vc14-64; run fetch-occt.ps1 first.")
endif()
add_custom_command(TARGET step2glb POST_BUILD
  COMMAND ${CMAKE_COMMAND} -E copy_if_different $<TARGET_RUNTIME_DLLS:step2glb> ${OCCT_THIRDPARTY_DLLS}
          "${OpenCASCADE_INSTALL_PREFIX}/OCCT_LGPL_EXCEPTION.txt" "${OpenCASCADE_INSTALL_PREFIX}/LICENSE_LGPL_21.txt"
          $<TARGET_FILE_DIR:step2glb>
  COMMAND_EXPAND_LISTS)
