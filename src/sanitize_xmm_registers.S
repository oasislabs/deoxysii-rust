#if defined(__WIN32__) || defined(__APPLE__)
# define cdecl(s) _##s
#else
# define cdecl(s) s
#endif
.text
.global cdecl(sanitize_xmm_registers)
cdecl(sanitize_xmm_registers):
  pxor %xmm0, %xmm0
  pxor %xmm1, %xmm1
  pxor %xmm2, %xmm2
  pxor %xmm3, %xmm3
  pxor %xmm4, %xmm4
  pxor %xmm5, %xmm5
  pxor %xmm6, %xmm6
  pxor %xmm7, %xmm7
  pxor %xmm8, %xmm8
  pxor %xmm9, %xmm9
  pxor %xmm10, %xmm10
  pxor %xmm11, %xmm11
  pxor %xmm12, %xmm12
  pxor %xmm13, %xmm13
  pxor %xmm14, %xmm14
  pxor %xmm15, %xmm15
  ret
