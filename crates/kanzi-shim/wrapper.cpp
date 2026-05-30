// C-style in-memory wrapper around kanzi-cpp's C API (src/api/Compressor.hpp,
// Decompressor.hpp). kanzi's C API needs a real file descriptor (it calls
// fileno()/fstat() and reads/writes the fd directly), so fmemopen() will not
// work. We back the FILE* with an unlinked mkstemp() temp file and move bytes
// at the fd level, exposing an lzav-style buffer API:
//   preallocate `dst` to kanzishim_bound(srclen); the call returns the length.
//
// Level -> (transform, entropy) mirrors kanzi's
// BlockCompressor::getTransformAndCodec for the pinned commit.

#define _POSIX_C_SOURCE 200809L
#include <cstddef>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <unistd.h>

#include "api/Compressor.hpp"
#include "api/Decompressor.hpp"

namespace {

const unsigned int MAX_BLOCK =
    16u * 1024u * 1024u; // 16 MiB; also decompress read buffer

void level_params(int level, char *transform, char *entropy) {
  const char *t = "DNA+LZ";
  const char *e = "HUFFMAN";
  switch (level) {
  case 0:
    t = "NONE";
    e = "NONE";
    break;
  case 1:
    t = "LZX";
    e = "NONE";
    break;
  case 2:
    t = "DNA+LZ";
    e = "HUFFMAN";
    break;
  case 3:
    t = "TEXT+UTF+PACK+MM+LZX";
    e = "HUFFMAN";
    break;
  case 4:
    t = "TEXT+UTF+EXE+PACK+MM+ROLZ";
    e = "NONE";
    break;
  case 5:
    t = "TEXT+UTF+BWT+RANK+ZRLT";
    e = "ANS0";
    break;
  case 6:
    t = "TEXT+UTF+BWT+SRT+ZRLT";
    e = "FPAQ";
    break;
  case 7:
    t = "LZP+TEXT+UTF+BWT+LZP";
    e = "CM";
    break;
  case 8:
    t = "EXE+RLT+TEXT+UTF+DNA";
    e = "TPAQ";
    break;
  case 9:
    t = "EXE+RLT+TEXT+UTF+DNA";
    e = "TPAQX";
    break;
  default:
    break;
  }
  std::strncpy(transform, t, 63);
  std::strncpy(entropy, e, 15);
}

// Create an unlinked temp file; returns an open fd (or -1) and a FILE* via
// *out.
int open_tmp(FILE **out, const char *mode) {
  char tmpl[] = "/tmp/kanzishimXXXXXX";
  int fd = mkstemp(tmpl);
  if (fd < 0)
    return -1;
  unlink(tmpl); // auto-removed on close
  FILE *f = fdopen(fd, mode);
  if (f == nullptr) {
    close(fd);
    return -1;
  }
  *out = f;
  return fd;
}

} // namespace

extern "C" {

size_t kanzishim_bound(size_t srclen) {
  // kanzi adds a small per-block header; NONE entropy may not shrink. Be
  // generous.
  return srclen + srclen / 2 + 64 * 1024;
}

long kanzishim_compress(const unsigned char *src, size_t srclen,
                        unsigned char *dst, size_t dstcap, int level) {
  FILE *f = nullptr;
  int fd = open_tmp(&f, "w+b");
  if (fd < 0)
    return -1;

  cData p;
  std::memset(&p, 0, sizeof(p));
  level_params(level, p.transform, p.entropy);
  size_t bs = srclen < 65536 ? 65536 : srclen;
  if (bs > MAX_BLOCK)
    bs = MAX_BLOCK;
  p.blockSize = bs;
  p.jobs = 1;
  p.checksum = 0;
  p.headerless = 0;

  cContext *ctx = nullptr;
  if (initCompressor(&p, f, &ctx) != 0) {
    std::fclose(f);
    return -2;
  }

  size_t off = 0;
  while (off < srclen) {
    size_t chunk = srclen - off;
    if (chunk > p.blockSize)
      chunk = p.blockSize;
    size_t outSize = 0;
    if (compress(ctx, src + off, chunk, &outSize) != 0) {
      disposeCompressor(&ctx, &outSize);
      std::fclose(f);
      return -3;
    }
    off += chunk;
  }
  size_t flushed = 0;
  disposeCompressor(&ctx, &flushed); // flushes the fd via raw write()

  off_t size = lseek(fd, 0, SEEK_END);
  if (size < 0 || (size_t)size > dstcap) {
    std::fclose(f);
    return -4;
  }
  lseek(fd, 0, SEEK_SET);
  ssize_t got = read(fd, dst, (size_t)size);
  std::fclose(f);
  return (got == size) ? (long)got : -5;
}

long kanzishim_decompress(const unsigned char *src, size_t srclen,
                          unsigned char *dst, size_t dstcap) {
  FILE *f = nullptr;
  int fd = open_tmp(&f, "w+b");
  if (fd < 0)
    return -1;

  if (write(fd, src, srclen) != (ssize_t)srclen) {
    std::fclose(f);
    return -2;
  }
  lseek(fd, 0, SEEK_SET);

  dData p;
  std::memset(&p, 0, sizeof(p));
  p.bufferSize = MAX_BLOCK;
  p.jobs = 1;
  p.headerless = 0;

  dContext *ctx = nullptr;
  if (initDecompressor(&p, f, &ctx) != 0) {
    std::fclose(f);
    return -3;
  }

  size_t off = 0;
  for (;;) {
    size_t inSize = 0;
    size_t outSize = dstcap - off; // remaining capacity = max block to produce
    if (decompress(ctx, dst + off, &inSize, &outSize) != 0) {
      disposeDecompressor(&ctx);
      std::fclose(f);
      return -4;
    }
    if (outSize == 0)
      break;
    off += outSize;
    if (off >= dstcap)
      break;
  }
  disposeDecompressor(&ctx);
  std::fclose(f);
  return (long)off;
}

} // extern "C"
