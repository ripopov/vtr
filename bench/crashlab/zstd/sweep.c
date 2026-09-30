// Size/time sweep: one frame vs flush every N vs independent frame every N.
#define _GNU_SOURCE
#include <zstd.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

static double now(void){struct timespec t;clock_gettime(CLOCK_MONOTONIC,&t);return t.tv_sec+t.tv_nsec*1e-9;}
static unsigned char *out; static size_t outcap;

/* mode 0: continue, end at the end; 1: flush every N; 2: end frame every N */
static size_t run(ZSTD_CCtx*c,const unsigned char*src,size_t n,size_t step,int mode,int level,int chk,double*sec){
  ZSTD_CCtx_reset(c,ZSTD_reset_session_and_parameters);
  ZSTD_CCtx_setParameter(c,ZSTD_c_compressionLevel,level);
  ZSTD_CCtx_setParameter(c,ZSTD_c_checksumFlag,chk);
  ZSTD_outBuffer o={out,outcap,0};
  double t0=now();
  for(size_t off=0;off<n;off+=step){
    size_t len=n-off<step?n-off:step;
    ZSTD_inBuffer in={src+off,len,0};
    ZSTD_EndDirective d = mode==0 ? (off+len==n?ZSTD_e_end:ZSTD_e_continue) : mode==1 ? (off+len==n?ZSTD_e_end:ZSTD_e_flush) : ZSTD_e_end;
    size_t r;
    do { r=ZSTD_compressStream2(c,&o,&in,d); if(ZSTD_isError(r)){fprintf(stderr,"%s\n",ZSTD_getErrorName(r));exit(1);} }
    while(d==ZSTD_e_continue? in.pos<in.size : r!=0);
  }
  *sec=now()-t0;
  return o.pos;
}

int main(int argc,char**argv){
  FILE*f=fopen(argv[1],"rb"); fseek(f,0,SEEK_END); size_t n=ftell(f); rewind(f);
  unsigned char*src=malloc(n); if(fread(src,1,n,f)!=n) return 1; fclose(f);
  outcap=ZSTD_compressBound(n)+(n/1024)*32; out=malloc(outcap);
  ZSTD_CCtx*c=ZSTD_createCCtx();
  int levels[]={1,3};
  size_t steps[]={4<<10,16<<10,64<<10,256<<10,1<<20,4<<20};
  for(int li=0;li<2;li++){
    int L=levels[li]; double s;
    size_t base=run(c,src,n,1<<20,0,L,0,&s); double bs=s;
    printf("%s L%d one-frame: %zu bytes ratio %.2f  %.0f MB/s\n",argv[1],L,base,(double)n/base,n/bs/1e6);
    printf("  %-8s | %-26s | %-26s | %-26s\n","N","flush every N","frame every N (no chk)","frame every N (+XXH32)");
    for(int si=0;si<6;si++){
      double s1,s2,s3;
      size_t a=run(c,src,n,steps[si],1,L,0,&s1);
      size_t b=run(c,src,n,steps[si],2,L,0,&s2);
      size_t d=run(c,src,n,steps[si],2,L,1,&s3);
      printf("  %6zuK | %10zu %+6.2f%% %4.0fMB/s | %10zu %+6.2f%% %4.0fMB/s | %10zu %+6.2f%% %4.0fMB/s\n",steps[si]>>10,
        a,100.0*(a-(double)base)/base,n/s1/1e6, b,100.0*(b-(double)base)/base,n/s2/1e6, d,100.0*(d-(double)base)/base,n/s3/1e6);
    }
  }
  return 0;
}
