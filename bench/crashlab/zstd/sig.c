#define _GNU_SOURCE
#define ZSTD_STATIC_LINKING_ONLY
#include <zstd.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <fcntl.h>
#include <stdatomic.h>
extern void*__libc_malloc(size_t); extern void*__libc_calloc(size_t,size_t); extern void __libc_free(void*); extern void*__libc_realloc(void*,size_t);
static atomic_long nmalloc; static int counting;
void*malloc(size_t s){ if(counting) nmalloc++; return __libc_malloc(s);} 
void*calloc(size_t a,size_t b){ if(counting) nmalloc++; return __libc_calloc(a,b);} 
void*realloc(void*p,size_t s){ if(counting) nmalloc++; return __libc_realloc(p,s);} 
void free(void*p){ if(counting&&p) nmalloc++; __libc_free(p);} 
static ZSTD_CCtx*sc; static unsigned char pending[1<<20]; static size_t npend; static unsigned char cbuf[ZSTD_COMPRESSBOUND(1<<20)]; static int fd;
static void onsegv(int s){ (void)s;
  counting=1; size_t r=ZSTD_compress2(sc,cbuf,sizeof cbuf,pending,npend); counting=0;
  if(!ZSTD_isError(r)) (void)!write(fd,cbuf,r);
  char m[160]; int k=snprintf(m,sizeof m,"handler: compressed %zu -> %zu bytes, allocator calls inside=%ld\n",npend,r,(long)nmalloc); (void)!write(2,m,k);
  _exit(3);}
int main(int argc,char**argv){
  for(int L=1;L<=3;L+=2) fprintf(stderr,"L%d: estimateCCtxSize=%zu estimateCStreamSize=%zu\n",L,ZSTD_estimateCCtxSize(L),ZSTD_estimateCStreamSize(L));
  ZSTD_compressionParameters cp=ZSTD_getCParams(3,1<<20,0);
  size_t ws=ZSTD_estimateCCtxSize_usingCParams(cp); fprintf(stderr,"L3 cparams for 1 MiB src: wlog=%u, estimate=%zu\n",cp.windowLog,ws);
  void*mem=aligned_alloc(64,(ws+63)&~63ul); sc=ZSTD_initStaticCCtx(mem,ws);
  ZSTD_CCtx_setParameter(sc,ZSTD_c_compressionLevel,3); ZSTD_CCtx_setParameter(sc,ZSTD_c_checksumFlag,1);
  fprintf(stderr,"static nbWorkers=2 -> %s\n",ZSTD_getErrorName(ZSTD_CCtx_setParameter(sc,ZSTD_c_nbWorkers,2)));
  FILE*f=fopen(argv[1],"rb"); npend=fread(pending,1,sizeof pending,f); fclose(f);
  /* warm-up run outside handler, counting allocations too */
  counting=1; size_t r=ZSTD_compress2(sc,cbuf,sizeof cbuf,pending,npend); counting=0;
  fprintf(stderr,"normal call on static ctx: %zu -> %s%zu, allocator calls=%ld\n",npend,ZSTD_isError(r)?ZSTD_getErrorName(r):"",r,(long)nmalloc);
  nmalloc=0; ZSTD_CCtx*dc=ZSTD_createCCtx(); counting=1; r=ZSTD_compress2(dc,cbuf,sizeof cbuf,pending,npend); counting=0;
  fprintf(stderr,"first call on heap ctx: allocator calls=%ld\n",(long)nmalloc); nmalloc=0;
  /* static ctx too small for level 19 */
  ZSTD_CCtx_setParameter(sc,ZSTD_c_compressionLevel,19); r=ZSTD_compress2(sc,cbuf,sizeof cbuf,pending,npend);
  fprintf(stderr,"static ctx sized for L3, asked L19: %s\n",ZSTD_isError(r)?ZSTD_getErrorName(r):"ok");
  ZSTD_CCtx_setParameter(sc,ZSTD_c_compressionLevel,3);
  fd=open("crash.zst",O_WRONLY|O_CREAT|O_TRUNC,0644);
  struct sigaction sa={0}; sa.sa_handler=onsegv; sigaction(SIGSEGV,&sa,0);
  *(volatile int*)0=1; return 0; }
