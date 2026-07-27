cmake_minimum_required(VERSION 3.21)
project(fcitx5-natsu-typeless VERSION 0.1.0 LANGUAGES CXX)

find_package(Fcitx5Core 5.1 REQUIRED)
find_package(Fcitx5Config 5.1 REQUIRED)
find_package(Fcitx5Module REQUIRED COMPONENTS DBus)
include("${FCITX_INSTALL_CMAKECONFIG_DIR}/Fcitx5Utils/Fcitx5CompilerSettings.cmake")

add_library(natsutypeless SHARED
    src/natsutypeless.cpp
    src/natsutypeless.h
)
target_compile_features(natsutypeless PRIVATE cxx_std_20)
target_link_libraries(natsutypeless
    PRIVATE
        Fcitx5::Core
        Fcitx5::Config
        Fcitx5::Module::DBus
)

install(TARGETS natsutypeless DESTINATION "${FCITX_INSTALL_LIBDIR}/fcitx5")
install(
    FILES natsutypeless.conf
    DESTINATION "${FCITX_INSTALL_PKGDATADIR}/addon"
)
