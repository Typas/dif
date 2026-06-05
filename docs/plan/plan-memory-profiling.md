# Plan: add memory profiling feature on `just bench-codecs` and `just bench-formats`

## Goal
`just bench-codecs` have introduced peak memory usage while the result is meaningless without correct profiling. The target is to use memory profiling tools to handle the memory usage statistics (mainly max and mean). The memory profiler should record the moment with the highest memory usage. All the new codes are in python.

## Allocator
- [mimalloc](https://github.com/microsoft/mimalloc)
- [jemalloc](https://github.com/jemalloc/jemalloc)
- [tcmalloc](https://github.com/google/tcmalloc)

## Profiling tools
- [memray](https://github.com/bloomberg/memray)
- maybe more

## Python rewiring

## Justfile rewiring
The environmental variable needs to be set under justfile.
