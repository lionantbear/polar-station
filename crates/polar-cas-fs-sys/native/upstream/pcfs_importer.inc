/* Polar bridge, compiled INSIDE pinned lib/inode.c to reuse its private encoder.
 * This is an input adapter, not an on-disk EROFS encoder. */
static int pcfs_write_file(struct erofs_importer *im, struct erofs_inode *inode)
{
    struct erofs_vfile *vf = inode->pcfs_vf;
    struct stat st;
    int ret = EROFS_RETVAL_FALLBACK, check;

    if (inode->sbi->available_compr_algs) {
        void *ictx = erofs_prepare_compressed_file(im, inode);
        if (IS_ERR(ictx)) { ret = PTR_ERR(ictx); goto out; }
        erofs_bind_compressed_file_with_vfile(ictx, vf, 0);
        ret = erofs_begin_compressed_file(ictx);
        if (ret) goto out;
        ret = erofs_write_compressed_file(ictx);
    }
    if (ret == EROFS_RETVAL_FALLBACK) {
        if (erofs_io_lseek(vf, 0, SEEK_SET) != 0) { ret = -EIO; goto out; }
        inode->datalayout = EROFS_INODE_FLAT_INLINE;
        ret = erofs_write_unencoded_data(inode, vf, 0, true, false);
    }
    /* Also checks that an FD source did not change while being encoded. */
    check = erofs_io_fstat(vf, &st);
    if (!ret && check) ret = check;
out:
    erofs_io_close(vf);
    inode->pcfs_vf = NULL;
    return ret;
}
