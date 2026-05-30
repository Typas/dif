/* Non-inline shim over lzav.h's inline API so dif-core can link real symbols.
   lzav-1 = lzav_compress_default (the study's chosen variant). */
#include "lzav.h"

int lzavshim_bound(int srclen) { return lzav_compress_bound(srclen); }

int lzavshim_compress(const void *src, void *dst, int srclen, int dstlen) {
  return lzav_compress_default(src, dst, srclen, dstlen);
}

int lzavshim_decompress(const void *src, void *dst, int srclen, int dstlen) {
  return lzav_decompress(src, dst, srclen, dstlen);
}
