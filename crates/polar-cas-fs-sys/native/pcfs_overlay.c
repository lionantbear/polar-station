#define _GNU_SOURCE
#include "pcfs_overlay.h"
#include <errno.h>
#include <fcntl.h>
#include <linux/mount.h>
#include <stdio.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <unistd.h>
#include <string.h>
static int32_t error_status(int e,int32_t *out){
    if(out)*out=e;
    if(e==EPERM||e==EACCES)return PCFS_VERITY_DENIED;
    if(e==ENOSYS||e==ENODEV||e==ENOTBLK||e==EOPNOTSUPP||e==EXDEV)return PCFS_UNSUPPORTED;
    if(e==ENFILE||e==EMFILE)return PCFS_LIMIT;
    return PCFS_IO;
}
static int directory(int fd,struct stat *st){
    int r;do{r=fstat(fd,st);}while(r<0&&errno==EINTR);
    if(r<0)return -1;
    if(!S_ISDIR(st->st_mode)){errno=ENOTDIR;return -1;}return 0;
}
static int same(const struct stat *a,const struct stat *b){return a->st_dev==b->st_dev&&a->st_ino==b->st_ino;}
#if defined(SYS_fsopen) && defined(SYS_fsconfig) && defined(SYS_fsmount)
static long config(int c,long op,const char *key,const char *value,long aux){
    long r;do{r=syscall(SYS_fsconfig,(long)c,op,key,value,aux);}while(r<0&&errno==EINTR);return r;
}
#endif
int32_t pcfs_overlay_mount_detached(int32_t lower,const int32_t *data,size_t count,
    int32_t upper,int32_t work,uint32_t exec,int32_t *out,int32_t *os_error){
    if(out)*out=-1;
    if(os_error)*os_error=0;
    if(!out||lower<0||(!data&&count)||count>PCFS_OVERLAY_MAX_DATADIRS||exec>1
        ||upper< -1||work< -1||(upper<0)!=(work<0))return PCFS_INVALID;
    struct stat ls,us,ws,ds[PCFS_OVERLAY_MAX_DATADIRS];
    if(directory(lower,&ls))return error_status(errno,os_error);
    for(size_t i=0;i<count;++i){
        if(directory(data[i],&ds[i]))return error_status(errno,os_error);
        if(same(&ls,&ds[i]))return PCFS_INVALID;
        for(size_t j=0;j<i;++j)if(same(&ds[i],&ds[j]))return PCFS_INVALID;
    }
    if(upper>=0){
        if(directory(upper,&us)||directory(work,&ws))return error_status(errno,os_error);
        if(us.st_dev!=ws.st_dev)return error_status(EXDEV,os_error);
        if(same(&us,&ws)||same(&ls,&us)||same(&ls,&ws))return PCFS_INVALID;
        for(size_t i=0;i<count;++i)if(same(&ds[i],&us)||same(&ds[i],&ws))return PCFS_INVALID;
    }
#if defined(SYS_fsopen) && defined(SYS_fsconfig) && defined(SYS_fsmount)
    long c,m;int e;unsigned long attrs=MOUNT_ATTR_NOSUID|MOUNT_ATTR_NODEV;
    if(upper<0)attrs|=MOUNT_ATTR_RDONLY;
    if(!exec)attrs|=MOUNT_ATTR_NOEXEC;
    do{c=syscall(SYS_fsopen,"overlay",(long)FSOPEN_CLOEXEC);}while(c<0&&errno==EINTR);
    if(c<0)return error_status(errno,os_error);
    if(config((int)c,FSCONFIG_SET_FD,"lowerdir+",NULL,lower)<0)goto fail;
    if(config((int)c,FSCONFIG_SET_STRING,"metacopy",count?"on":"off",0)<0)goto fail;
    /* follow conflicts with metacopy=on when an upper is present. */
    if(config((int)c,FSCONFIG_SET_STRING,"redirect_dir",upper>=0?"on":"follow",0)<0)goto fail;
    if(config((int)c,FSCONFIG_SET_STRING,"index","off",0)<0)goto fail;
    if(config((int)c,FSCONFIG_SET_STRING,"nfs_export","off",0)<0)goto fail;
    for(size_t i=0;i<count;++i)if(config((int)c,FSCONFIG_SET_FD,"datadir+",NULL,data[i])<0)goto fail;
    if(upper>=0){
        if(config((int)c,FSCONFIG_SET_FD,"upperdir",NULL,upper)<0)goto fail;
        if(config((int)c,FSCONFIG_SET_FD,"workdir",NULL,work)<0)goto fail;
    }
    if(config((int)c,FSCONFIG_CMD_CREATE,NULL,NULL,0)<0)goto fail;
    do{m=syscall(SYS_fsmount,c,(long)FSMOUNT_CLOEXEC,(long)attrs);}while(m<0&&errno==EINTR);
    if(m<0)goto fail;
    close((int)c);*out=(int32_t)m;return PCFS_OK;
fail:e=errno;close((int)c);return error_status(e,os_error);
#else
    return error_status(ENOSYS,os_error);
#endif
}
static int child_directory(int parent,const char *name){
    int r;do{r=mkdirat(parent,name,0700);}while(r<0&&errno==EINTR);
    if(r<0&&errno!=EEXIST)return -1;
    do{r=openat(parent,name,O_RDONLY|O_DIRECTORY|O_NOFOLLOW|O_CLOEXEC);}while(r<0&&errno==EINTR);
    return r;
}
int32_t pcfs_frozen_link_content(int32_t root,int32_t source,const uint8_t digest[32],uint64_t size,int32_t *os_error){
    if(os_error)*os_error=0;
    if(root<0||source<0||!digest)return PCFS_INVALID;
    struct stat st;struct pcfs_verity_digest_v1 measured;
    if(directory(root,&st))return error_status(errno,os_error);
    if(st.st_uid!=geteuid()||(st.st_mode&0077))return PCFS_INVALID;
    int32_t rc=pcfs_verity_require(source,0,&measured,os_error);if(rc)return rc;
    if(fstat(source,&st)<0)return error_status(errno,os_error);
    if(!S_ISREG(st.st_mode)||st.st_size<0||(uint64_t)st.st_size!=size)return PCFS_CORRUPT;
    char path[PCFS_SHA256_PATH_CAPACITY],fdpath[64],shard[3];
    rc=pcfs_sha256_path(digest,path,sizeof(path));if(rc)return rc;
    memcpy(shard,path+15,2);shard[2]=0;
    int obj=-1,alg=-1,dir=-1,e=0;
    obj=child_directory(root,"objects");if(obj<0)goto fail;
    alg=child_directory(obj,"sha256");if(alg<0)goto fail;
    dir=child_directory(alg,shard);if(dir<0)goto fail;
    snprintf(fdpath,sizeof(fdpath),"/proc/self/fd/%d",source);
    int r;do{r=linkat(AT_FDCWD,fdpath,dir,path+18,AT_SYMLINK_FOLLOW);}while(r<0&&errno==EINTR);
    if(r<0)goto fail;
    close(dir);close(alg);close(obj);return PCFS_OK;
fail:e=errno;if(dir>=0)close(dir);if(alg>=0)close(alg);if(obj>=0)close(obj);return error_status(e,os_error);
}

/* M5: per-mount write barrier. Caller owns all writable aliases and upper/work.
 * It must not create writable clone mounts. This is NOT an application-level
 * DB transaction boundary; the application must finish/flush its own writes. */
#include <sys/vfs.h>
#include <linux/magic.h>
int32_t pcfs_overlay_readonly_v1(int32_t fd, int32_t *os_error) {
    if (os_error) *os_error=0;
    if (fd<0) return PCFS_INVALID;
    struct statfs fs;
    if (fstatfs(fd,&fs)<0) return error_status(errno,os_error);
    if ((unsigned long)fs.f_type!=(unsigned long)OVERLAYFS_SUPER_MAGIC) return PCFS_INVALID;
#if defined(SYS_mount_setattr)
    struct mount_attr attr;
    memset(&attr,0,sizeof(attr));
    attr.attr_set=MOUNT_ATTR_RDONLY;
    long rc;
    do { rc=syscall(SYS_mount_setattr,(long)fd,"",(long)AT_EMPTY_PATH,&attr,sizeof(attr)); }
    while(rc<0 && errno==EINTR);
    if(rc<0) return error_status(errno,os_error);
    /* Mount FDs can be O_PATH; syncfs requires a real opened directory. */
    int root;
    do { root=openat(fd,".",O_RDONLY|O_DIRECTORY|O_CLOEXEC); } while(root<0 && errno==EINTR);
    if(root<0) return error_status(errno,os_error);
    int result;
    do { result=syncfs(root); } while(result<0 && errno==EINTR);
    int e=errno;
    close(root);
    if(result<0) return error_status(e,os_error);
    return PCFS_OK;
#else
    return error_status(ENOSYS,os_error);
#endif
}
