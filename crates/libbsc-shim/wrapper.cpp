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
  static int rc = bsc_init(LIBBSC_FEATURE_FASTMODE);
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
