cmake_minimum_required(VERSION 3.22)

project(easymanage_native LANGUAGES CXX)

add_library(easymanage_native INTERFACE)
target_compile_features(easymanage_native INTERFACE cxx_std_20)

