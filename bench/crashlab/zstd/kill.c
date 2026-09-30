#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <fcntl.h>
#include <signal.h>
#include <sys/mman.h>
#include <sys/wait.h>
#include <sys/stat.h>
#include <time.h>
int main(int argc,char**argv){
  const char*path=argv[1]; size_t big=(size_t)1<<30;
  for(int trial=0;trial<3;trial++){
    unlink(path);
    pid_t p=fork();
    if(!p){ char*b=malloc(big); memset(b,'x',big); int fd=open(path,O_WRONLY|O_CREAT|O_TRUNC|O_APPEND,0644);
      (void)!write(fd,"HEAD",4); kill(getppid(),SIGUSR1); ssize_t r=write(fd,b,big); fprintf(stderr,"child write returned %zd\n",r); _exit(0);}
    sigset_t s; sigemptyset(&s); sigaddset(&s,SIGUSR1); sigprocmask(SIG_BLOCK,&s,0); int sig; sigwait(&s,&sig);
    struct timespec ts={0,(trial+1)*40*1000000L}; nanosleep(&ts,0); kill(p,SIGKILL); int st; waitpid(p,&st,0);
    struct stat sb; stat(path,&sb); printf("trial %d: SIGKILL %d ms into a 1 GiB write(): file size %lld (%s)\n",trial,(trial+1)*40,(long long)sb.st_size,
      sb.st_size==4+(long long)big?"complete":"SHORT/torn");
  }
  /* MAP_SHARED survives SIGKILL */
  unlink(path); int fd=open(path,O_RDWR|O_CREAT,0644); ftruncate(fd,1<<20);
  pid_t p=fork(); if(!p){ char*m=mmap(0,1<<20,PROT_READ|PROT_WRITE,MAP_SHARED,fd,0); memcpy(m+12345,"mmap-survives",13); raise(SIGKILL);} 
  int st; waitpid(p,&st,0); char buf[14]={0}; pread(fd,buf,13,12345); printf("after child SIGKILL, MAP_SHARED bytes read back: '%s'\n",buf);
  return 0; }
