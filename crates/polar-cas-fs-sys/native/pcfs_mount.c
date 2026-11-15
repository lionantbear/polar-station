#define _GNU_SOURCE
#include "pcfs_mount.h"
#include <errno.h>
#include <fcntl.h>
#include <linux/mount.h>
#include <stdio.h>
#include <sys/syscall.h>
#include <unistd.h>

static int32_t mount_error(int error, int32_t *os_error)
{
    if (os_error) *os_error=error;
    if (error==ENOSYS||error==ENODEV||error==ENOTBLK||error==EOPNOTSUPP) return PCFS_UNSUPPORTED;
    if (error==EPERM||error==EACCES) return PCFS_VERITY_DENIED;
    if (error==EMFILE||error==ENFILE) return PCFS_LIMIT;
    return PCFS_IO;
}
#if defined(SYS_fsopen) && defined(SYS_fsconfig) && defined(SYS_fsmount)
static long config(int context, long cmd, const char *key, const char *value)
{
    long r;
    do {r=syscall(SYS_fsconfig,(long)context,cmd,key,value,0L);}while(r<0&&errno==EINTR);
    return r;
}
static int config_fd_path(int context, const char *key, int fd)
{
    char path[64]; int len=snprintf(path,sizeof(path),"/proc/self/fd/%d",fd);
    if (len<0||(size_t)len>=sizeof(path)) {errno=EINVAL;return -1;}
    return (int)config(context,FSCONFIG_SET_STRING,key,path);
}
#endif
int32_t pcfs_native_mount_detached(int32_t meta_fd, const int32_t *pack_fds,
    size_t pack_count, uint32_t allow_exec, int32_t *mount_fd, int32_t *os_error)
{
    size_t i; struct pcfs_verity_digest_v1 measured; int32_t ret;
    if (mount_fd) *mount_fd=-1;
    if (os_error) *os_error=0;
    if (!mount_fd||meta_fd<0||(!pack_fds&&pack_count)||pack_count>64||allow_exec>1) return PCFS_INVALID;
    ret=pcfs_verity_require(meta_fd,0,&measured,os_error);
    if (ret) return ret;
    for(i=0;i<pack_count;++i) {
        ret=pcfs_verity_require(pack_fds[i],0,&measured,os_error);
        if(ret) return ret;
    }
#if defined(SYS_fsopen) && defined(SYS_fsconfig) && defined(SYS_fsmount)
    {
        long context, mounted; int error; unsigned long attrs=MOUNT_ATTR_RDONLY|MOUNT_ATTR_NOSUID|MOUNT_ATTR_NODEV;
        do {context=syscall(SYS_fsopen,"erofs",(long)FSOPEN_CLOEXEC);}while(context<0&&errno==EINTR);
        if(context<0) return mount_error(errno,os_error);
        if(config_fd_path((int)context,"source",meta_fd)<0) goto fail;
        for(i=0;i<pack_count;++i) if(config_fd_path((int)context,"device",pack_fds[i])<0) goto fail;
        if(config((int)context,FSCONFIG_CMD_CREATE,NULL,NULL)<0) goto fail;
        if(!allow_exec) attrs|=MOUNT_ATTR_NOEXEC;
        do {mounted=syscall(SYS_fsmount,context,(long)FSMOUNT_CLOEXEC,(long)attrs);}while(mounted<0&&errno==EINTR);
        if(mounted<0) goto fail;
        close((int)context); *mount_fd=(int32_t)mounted; return PCFS_OK;
fail:
        error=errno;close((int)context);return mount_error(error,os_error);
    }
#else
    return mount_error(ENOSYS,os_error);
#endif
}
