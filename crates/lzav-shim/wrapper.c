/* Non-inline shim over lzav.h's inline API so dif-core can link real symbols.
   lzav-1 = lzav_compress_default, lzav-2 = lzav_compress_hi (high ratio).
   Decompression is format-tagged, so one entry point decodes either. */
#include "lzav.h"

int lzavshim_bound(int srclen) { return lzav_compress_bound(srclen); }

int lzavshim_bound_hi(int srclen) { return lzav_compress_bound_hi(srclen); }

int lzavshim_compress(const void *src, void *dst, int srclen, int dstlen) {
  return lzav_compress_default(src, dst, srclen, dstlen);
}

int lzavshim_compress_hi(const void *src, void *dst, int srclen, int dstlen) {
  return lzav_compress_hi(src, dst, srclen, dstlen);
}

int lzavshim_decompress(const void *src, void *dst, int srclen, int dstlen) {
  return lzav_decompress(src, dst, srclen, dstlen);
}
