/* Thin extern-"C" shim over libbsc's block API so dif-core can link real
 * symbols. CPU single-thread BWT. A 1-byte tag prefixes every blob:
 *   tag 1 = libbsc block follows; tag 0 = stored raw (incompressible / empty).
 * Decode sizes come from dif's known raw_len, so the blob is self-contained. */
#include <cstddef>
#include <cstring>

#include "libbsc/libbsc.h"

static int ensure_init() {
  // C++11 guarantees thread-safe init of this function-local static, so the
  // one-time bsc_init runs once even under dif's parallel frame compression.
  // A CUDA build (bench `--cuda`) must init with the CUDA feature so libbsc sets
  // up its GPU lock; it's a no-op for the CPU codecs and safe without a GPU
  // (device work is lazy in bsc_st_encode_cuda). dif-core never defines the
  // macro, so it stays pure FASTMODE.
  static int rc = bsc_init(LIBBSC_FEATURE_FASTMODE
#ifdef LIBBSC_CUDA_SUPPORT
                           | LIBBSC_FEATURE_CUDA
#endif
  );
  return rc;
}

// dif level -> libbsc QLFC coder: 1=fast, 2=static (default), 3=adaptive.
static int coder_for(int level) {
  if (level <= 1) return LIBBSC_CODER_QLFC_FAST;
  if (level >= 3) return LIBBSC_CODER_QLFC_ADAPTIVE;
  return LIBBSC_CODER_QLFC_STATIC;
}

extern "C" int bscshim_bound(int n) { return n + LIBBSC_HEADER_SIZE + 1; }

extern "C" int bscshim_compress(const unsigned char *src, int srclen,
                                unsigned char *dst, int dstcap, int level) {
  if (ensure_init() != LIBBSC_NO_ERROR) return -100;
  if (dstcap < srclen + LIBBSC_HEADER_SIZE + 1) return -101;
  if (srclen == 0) { // empty -> store raw (libbsc rejects zero-length blocks)
    dst[0] = 0;
    return 1;
  }
  int r = bsc_compress(src, dst + 1, srclen, LIBBSC_DEFAULT_LZPHASHSIZE,
                       LIBBSC_DEFAULT_LZPMINLEN, LIBBSC_BLOCKSORTER_BWT,
                       coder_for(level), LIBBSC_FEATURE_FASTMODE);
  if (r >= 0) {
    dst[0] = 1;
    return r + 1;
  }
  if (r == LIBBSC_NOT_COMPRESSIBLE) {
    dst[0] = 0;
    std::memcpy(dst + 1, src, srclen);
    return srclen + 1;
  }
  return r; // genuine error
}

extern "C" int bscshim_decompress(const unsigned char *src, int srclen,
                                  unsigned char *dst, int rawlen) {
  if (ensure_init() != LIBBSC_NO_ERROR) return -100;
  if (srclen < 1) return -101;
  if (src[0] == 0) { // stored raw
    int n = srclen - 1;
    if (n > rawlen) return -102;
    if (n > 0) std::memcpy(dst, src + 1, n);
    return n;
  }
  int blockSize = 0, dataSize = 0;
  int info = bsc_block_info(src + 1, LIBBSC_HEADER_SIZE, &blockSize, &dataSize,
                            LIBBSC_FEATURE_FASTMODE);
  if (info < LIBBSC_NO_ERROR) return info;
  if (dataSize > rawlen || blockSize > srclen - 1) return -103;
  int r = bsc_decompress(src + 1, blockSize, dst, dataSize,
                         LIBBSC_FEATURE_FASTMODE);
  if (r < LIBBSC_NO_ERROR) return r;
  return dataSize;
}

/* --- Parameterized path (bench only; dif-core never links these) -----------
 * py/bench explores libbsc's block-sorter (-m) and entropy coder (-e) knobs
 * plus CLI-style block chunking (-b), to inform dif's eventual `bsc` levels.
 * Mirrors bsc.cpp's compression loop: split the input into blockBytes-sized
 * chunks and bsc_compress each one (bsc_store the incompressible ones).
 * bsc_compress/bsc_store both emit a self-describing libbsc block (28-byte
 * header carrying its own compressed+raw sizes), so the concatenated stream
 * needs no extra framing -- the _ex decompressor walks it via bsc_block_info.
 * `blockSorter`/`coder`/`features` are passed straight through as LIBBSC_* enum
 * values (the bench owns the -m/-e digit -> enum mapping); `features` carries
 * LIBBSC_FEATURE_CUDA for ST7/ST8 on a CUDA build, else just FASTMODE. */

extern "C" int bscshim_bound_ex(int n, int blockBytes) {
  if (n <= 0) return 1;
  if (blockBytes <= 0) blockBytes = n;
  int nblocks = (n + blockBytes - 1) / blockBytes;
  return n + nblocks * (LIBBSC_HEADER_SIZE + 1) + 64; // worst case + slack
}

extern "C" int bscshim_compress_ex(const unsigned char *src, int srclen,
                                   unsigned char *dst, int dstcap, int blockBytes,
                                   int blockSorter, int coder, int features) {
  if (ensure_init() != LIBBSC_NO_ERROR) return -100;
  if (srclen <= 0) return 0; // empty -> empty stream
  if (blockBytes <= 0) blockBytes = srclen;
  int inPos = 0, outPos = 0;
  while (inPos < srclen) {
    int chunk = srclen - inPos;
    if (chunk > blockBytes) chunk = blockBytes;
    if (dstcap - outPos < chunk + LIBBSC_HEADER_SIZE) return -101;
    int r = bsc_compress(src + inPos, dst + outPos, chunk,
                         LIBBSC_DEFAULT_LZPHASHSIZE, LIBBSC_DEFAULT_LZPMINLEN,
                         blockSorter, coder, features);
    if (r == LIBBSC_NOT_COMPRESSIBLE) // store this block verbatim (still a
      r = bsc_store(src + inPos, dst + outPos, chunk, features); // libbsc block)
    if (r < LIBBSC_NO_ERROR) return r; // e.g. NOT_SUPPORTED for a CUDA-only -m
    outPos += r;
    inPos += chunk;
  }
  return outPos;
}

extern "C" int bscshim_decompress_ex(const unsigned char *src, int srclen,
                                     unsigned char *dst, int rawlen) {
  if (ensure_init() != LIBBSC_NO_ERROR) return -100;
  int inPos = 0, outPos = 0;
  while (inPos < srclen) {
    if (srclen - inPos < LIBBSC_HEADER_SIZE) return -102;
    int blockSize = 0, dataSize = 0;
    int info = bsc_block_info(src + inPos, LIBBSC_HEADER_SIZE, &blockSize,
                              &dataSize, LIBBSC_FEATURE_FASTMODE);
    if (info < LIBBSC_NO_ERROR) return info;
    if (blockSize > srclen - inPos || dataSize > rawlen - outPos) return -103;
    int r = bsc_decompress(src + inPos, blockSize, dst + outPos, dataSize,
                           LIBBSC_FEATURE_FASTMODE);
    if (r < LIBBSC_NO_ERROR) return r;
    inPos += blockSize;
    outPos += dataSize;
  }
  return outPos;
}
