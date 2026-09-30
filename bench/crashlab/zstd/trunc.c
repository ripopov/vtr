#include <zstd.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
static unsigned char*src,*out; static size_t n,cap;
/* streaming-decode buf[0..len): returns bytes emitted, prints stop reason, verifies prefix equals src */
static size_t decode(const unsigned char*buf,size_t len,const char*tag){
  ZSTD_DCtx*d=ZSTD_createDCtx(); size_t dcap=n+1; unsigned char*dst=malloc(dcap);
  ZSTD_inBuffer in={buf,len,0}; ZSTD_outBuffer o={dst,dcap,0}; size_t r=1;
  while(in.pos<in.size){ r=ZSTD_decompressStream(d,&o,&in); if(ZSTD_isError(r)) break; }
  int ok=memcmp(dst,src,o.pos)==0;
  printf("  %-34s in=%9zu consumed=%9zu out=%9zu prefix_ok=%d last_ret=%s%zu\n",tag,len,in.pos,o.pos,ok,
    ZSTD_isError(r)?ZSTD_getErrorName(r):"", ZSTD_isError(r)?0:r);
  ZSTD_freeDCtx(d); free(dst); return o.pos;
}
int main(int argc,char**argv){
  FILE*f=fopen(argv[1],"rb"); fseek(f,0,SEEK_END); n=ftell(f); rewind(f);
  src=malloc(n); if(fread(src,1,n,f)!=n) return 1; fclose(f); n=8<<20; /* use 8 MiB */
  cap=ZSTD_compressBound(n)*2; out=malloc(cap);
  size_t step=64<<10, nfl=n/step, *fo=malloc(sizeof(size_t)*(nfl+1));
  ZSTD_CCtx*c=ZSTD_createCCtx(); ZSTD_CCtx_setParameter(c,ZSTD_c_compressionLevel,3);
  ZSTD_CCtx_setParameter(c,ZSTD_c_checksumFlag,1);
  ZSTD_outBuffer o={out,cap,0};
  for(size_t i=0;i<nfl;i++){ ZSTD_inBuffer in={src+i*step,step,0};
    ZSTD_EndDirective e= i+1==nfl?ZSTD_e_end:ZSTD_e_flush; while(ZSTD_compressStream2(c,&o,&in,e)); fo[i]=o.pos; }
  printf("flush-every-64K frame, 8 MiB input -> %zu bytes, %zu flush points\n",o.pos,nfl);
  FILE*w=fopen("flushed.zst","wb"); fwrite(out,1,o.pos,w); fclose(w);
  decode(out,o.pos,"complete");
  decode(out,fo[nfl-2],"cut exactly at flush #126 (no end)");
  decode(out,fo[63],"cut exactly at flush #63");
  decode(out,fo[63]+1000,"cut 1000 B into next block");
  decode(out,o.pos-4,"cut checksum off (last 4 B)");
  /* bit flip inside a flushed block of unended frame */
  unsigned char*cp=malloc(fo[63]); memcpy(cp,out,fo[63]);
  int det=0,undet=0,wrong=0;
  for(int k=0;k<200;k++){ memcpy(cp,out,fo[63]); size_t p=fo[30]+10+ (size_t)k*97 % (fo[31]-fo[30]-20); cp[p]^=1<<(k%8);
    ZSTD_DCtx*d=ZSTD_createDCtx(); unsigned char*dst=malloc(n); ZSTD_inBuffer in={cp,fo[63],0}; ZSTD_outBuffer oo={dst,n,0}; size_t r=0;
    while(in.pos<in.size){ r=ZSTD_decompressStream(d,&oo,&in); if(ZSTD_isError(r)) break; }
    if(ZSTD_isError(r)) det++; else if(oo.pos==64*step && memcmp(dst,src,oo.pos)==0) undet++; else wrong++;
    free(dst); ZSTD_freeDCtx(d); }
  printf("200 single-bit flips in block 31 of an unended (flushed) frame: decoder error=%d, silent wrong output=%d, harmless=%d\n",det,wrong,undet);
  /* same flips, but frame complete with checksum */
  det=0;undet=0;wrong=0; unsigned char*cf=malloc(o.pos);
  for(int k=0;k<200;k++){ memcpy(cf,out,o.pos); size_t p=fo[30]+10+ (size_t)k*97 % (fo[31]-fo[30]-20); cf[p]^=1<<(k%8);
    ZSTD_DCtx*d=ZSTD_createDCtx(); unsigned char*dst=malloc(n+1); ZSTD_inBuffer in={cf,o.pos,0}; ZSTD_outBuffer oo={dst,n+1,0}; size_t r=0;
    while(in.pos<in.size){ r=ZSTD_decompressStream(d,&oo,&in); if(ZSTD_isError(r)) break; }
    if(ZSTD_isError(r)) det++; else if(memcmp(dst,src,n)==0) undet++; else wrong++;
    free(dst); ZSTD_freeDCtx(d); }
  printf("same flips, complete frame with checksum: decoder error=%d, silent wrong=%d, harmless=%d\n",det,wrong,undet);
  /* raw-ish data: incompressible block -> raw blocks, flips undetectable without checksum */
  return 0;
}
