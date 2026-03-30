use super::{
    get_block_cache, BlockDevice, DirEntry, DiskInode, DiskInodeType,
    EasyFileSystem, DIRENT_SZ,
};
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::{Mutex, MutexGuard};
/// Virtual filesystem layer over easy-fs
pub struct Inode {
    block_id: usize,
    block_offset: usize,
    fs: Arc<Mutex<EasyFileSystem>>,
    block_device: Arc<dyn BlockDevice>,
    inode_id: u32,
}

impl Inode {
    /// Create a vfs inode
    pub fn new(
        block_id: u32,
        block_offset: usize,
        fs: Arc<Mutex<EasyFileSystem>>,
        block_device: Arc<dyn BlockDevice>,
        inode_id: u32,
    ) -> Self {
        Self {
            block_id: block_id as usize,
            block_offset,
            fs,
            block_device,
            inode_id,
        }
    }
    /// Call a function over a disk inode to read it
    fn read_disk_inode<V>(&self, f: impl FnOnce(&DiskInode) -> V) -> V {
        get_block_cache(self.block_id, Arc::clone(&self.block_device))
            .lock()
            .read(self.block_offset, f)
    }
    /// Call a function over a disk inode to modify it
    fn modify_disk_inode<V>(&self, f: impl FnOnce(&mut DiskInode) -> V) -> V {
        get_block_cache(self.block_id, Arc::clone(&self.block_device))
            .lock()
            .modify(self.block_offset, f)
    }
    /// Find inode under a disk inode by name
    fn find_inode_id(&self, name: &str, disk_inode: &DiskInode) -> Option<u32> {
        // assert it is a directory
        assert!(disk_inode.is_dir());
        let file_count = (disk_inode.size as usize) / DIRENT_SZ;
        let mut dirent = DirEntry::empty();
        for i in 0..file_count {
            assert_eq!(
                disk_inode.read_at(DIRENT_SZ * i, dirent.as_bytes_mut(), &self.block_device,),
                DIRENT_SZ,
            );
            if dirent.name() == name {
                return Some(dirent.inode_id() as u32);
            }
        }
        None
    }
    /// Find inode under current inode by name
    pub fn find(&self, name: &str) -> Option<Arc<Inode>> {
        let fs = self.fs.lock();
        self.read_disk_inode(|disk_inode| {
            self.find_inode_id(name, disk_inode).map(|inode_id| {
                let (block_id, block_offset) = fs.get_disk_inode_pos(inode_id);
                Arc::new(Self::new(
                    block_id,
                    block_offset,
                    self.fs.clone(),
                    self.block_device.clone(),
                    inode_id,
                ))
            })
        })
    }
    /// Increase the size of a disk inode
    fn increase_size(
        &self,
        new_size: u32,
        disk_inode: &mut DiskInode,
        fs: &mut MutexGuard<EasyFileSystem>,
    ) {
        if new_size < disk_inode.size {
            return;
        }
        let blocks_needed = disk_inode.blocks_num_needed(new_size);
        let mut v: Vec<u32> = Vec::new();
        for _ in 0..blocks_needed {
            v.push(fs.alloc_data());
        }
        disk_inode.increase_size(new_size, v, &self.block_device);
    }
    /// Create inode under current inode by name
    pub fn create(&self, name: &str) -> Option<Arc<Inode>> {
        let mut fs = self.fs.lock();
        let op = |root_inode: &DiskInode| {
            // assert it is a directory
            assert!(root_inode.is_dir());
            // has the file been created?
            self.find_inode_id(name, root_inode)
        };
        if self.read_disk_inode(op).is_some() {
            return None;
        }
        // create a new file
        // alloc a inode with an indirect block
        let new_inode_id = fs.alloc_inode();
        // initialize inode
        let (new_inode_block_id, new_inode_block_offset) = fs.get_disk_inode_pos(new_inode_id);
        get_block_cache(new_inode_block_id as usize, Arc::clone(&self.block_device))
            .lock()
            .modify(new_inode_block_offset, |new_inode: &mut DiskInode| {
                new_inode.initialize(DiskInodeType::File);
            });
        self.modify_disk_inode(|root_inode| {
            // append file in the dirent
            let file_count = (root_inode.size as usize) / DIRENT_SZ;
            let new_size = (file_count + 1) * DIRENT_SZ;
            // increase size
            self.increase_size(new_size as u32, root_inode, &mut fs);
            // write dirent
            let dirent = DirEntry::new(name, new_inode_id);
            root_inode.write_at(
                file_count * DIRENT_SZ,
                dirent.as_bytes(),
                &self.block_device,
            );
        });

        let (block_id, block_offset) = fs.get_disk_inode_pos(new_inode_id);
        // Removed block_cache_sync_all() for performance optimization
        // block_cache_sync_all();
        // return inode
        Some(Arc::new(Self::new(
            block_id,
            block_offset,
            self.fs.clone(),
            self.block_device.clone(),
            new_inode_id,
        )))
        // release efs lock automatically by compiler
    }
    /// List inodes under current inode
    pub fn ls(&self) -> Vec<String> {
        let _fs = self.fs.lock();
        self.read_disk_inode(|disk_inode| {
            let file_count = (disk_inode.size as usize) / DIRENT_SZ;
            let mut v: Vec<String> = Vec::new();
            for i in 0..file_count {
                let mut dirent = DirEntry::empty();
                assert_eq!(
                    disk_inode.read_at(i * DIRENT_SZ, dirent.as_bytes_mut(), &self.block_device,),
                    DIRENT_SZ,
                );
                v.push(String::from(dirent.name()));
            }
            v
        })
    }
    /// Read data from current inode
    pub fn read_at(&self, offset: usize, buf: &mut [u8]) -> usize {
        let _fs = self.fs.lock();
        self.read_disk_inode(|disk_inode| disk_inode.read_at(offset, buf, &self.block_device))
    }
    /// Write data to current inode
    pub fn write_at(&self, offset: usize, buf: &[u8]) -> usize {
        let mut fs = self.fs.lock();
        let size = self.modify_disk_inode(|disk_inode| {
            self.increase_size((offset + buf.len()) as u32, disk_inode, &mut fs);
            disk_inode.write_at(offset, buf, &self.block_device)
        });
        // Removed block_cache_sync_all() for performance optimization
        // block_cache_sync_all();
        size
    }
    /// Clear the data in current inode
    pub fn clear(&self) {
        let mut fs = self.fs.lock();
        self.modify_disk_inode(|disk_inode| {
            let size = disk_inode.size;
            let data_blocks_dealloc = disk_inode.clear_size(&self.block_device);
            assert!(data_blocks_dealloc.len() == DiskInode::total_blocks(size) as usize);
            for data_block in data_blocks_dealloc.into_iter() {
                fs.dealloc_data(data_block);
            }
        });
        // Removed block_cache_sync_all() for performance optimization
        // block_cache_sync_all();
    }

    /// 提升diskinode的函数到这里
    /// 是否是目录
    pub fn is_dir(&self)->bool {
        self.read_disk_inode(|disk_inode| {
            disk_inode.is_dir()
        })
    }
    /// 返回inode_number
    pub fn get_inode_number(&self)->u32 {
        self.inode_id
    }
    /// 返回硬链接数
    pub fn nlink_counts(&self)->u32 {
        self.read_disk_inode(|disk_inode|{
            disk_inode.nlink
        })
    }

    /// Link file under current directory to another inode 
    pub fn link(&self,name:&str, inode_id:u32)->Option<Arc<Inode>>{
        let mut fs=self.fs.lock();
        //1. 检查是否覆盖
        if self.read_disk_inode(|root_inode|{
            self.find_inode_id(name, root_inode).is_some()
        }){
            return None;
        }

        //2.增加目标计数
        let (target_block_id, target_block_offset)=fs.get_disk_inode_pos(inode_id);
        get_block_cache(target_block_id as usize, Arc::clone(&self.block_device))
            .lock()
            .modify(target_block_offset, |target_inode:&mut DiskInode|{
                target_inode.nlink+=1;
            });
        // 3.修改当前目录内容
        self.modify_disk_inode(|root_inode| {
            let file_count=(root_inode.size as usize)/DIRENT_SZ;
            let new_size=(file_count+1)*DIRENT_SZ;
            self.increase_size(new_size as u32, root_inode, &mut fs);

            let dirent=DirEntry::new(name, inode_id);
            root_inode.write_at(file_count*DIRENT_SZ, dirent.as_bytes(), &self.block_device)
        });
        // Removed block_cache_sync_all() for performance optimization
        // block_cache_sync_all();
        Some(Arc::new(Self::new(target_block_id, target_block_offset, self.fs.clone(),self.block_device.clone(), inode_id)))
    }

    /// Unlink file under current inode by name 
    pub fn unlink(&self, name:&str)->isize {
        let fs = self.fs.lock();
        let mut target_inode_id:Option<u32>=None;
        let mut entry_offset:Option<usize>=None;

        //1.当前目录查找
        self.read_disk_inode(|root_inode| {
            let file_count=(root_inode.size as usize)/DIRENT_SZ;
            let mut dirent=DirEntry::empty();
            for i in 0..file_count {
                root_inode.read_at(i*DIRENT_SZ, dirent.as_bytes_mut(), &self.block_device);
                if dirent.name()==name{
                    target_inode_id=Some(dirent.inode_id());
                    entry_offset=Some(i*DIRENT_SZ);
                    break;
                }
            }
        });

        if target_inode_id.is_none() {
            return -1;
        }

        let inode_id=target_inode_id.unwrap();
        let (target_block_id, target_block_offset)=fs.get_disk_inode_pos(inode_id);
        let mut nlink_is_zero=false;
        // 2.修改nilink
        get_block_cache(target_block_id as usize, Arc::clone(&self.block_device))
            .lock()
            .modify(target_block_offset, |target_inode:&mut DiskInode|{
                if target_inode.nlink>0 {
                    target_inode.nlink-=1;
                    if target_inode.nlink==0 {
                        nlink_is_zero=true;
                    }
                }
            });
        // 3. 移除目录项
        self.modify_disk_inode(|root_inode| {
            let file_count=(root_inode.size as usize)/DIRENT_SZ;
            let last_entry_offset=(file_count-1)*DIRENT_SZ;

            if entry_offset.unwrap() !=last_entry_offset {
                let mut last_dirent=DirEntry::empty();
                root_inode.read_at(last_entry_offset, last_dirent.as_bytes_mut(), &self.block_device);
                root_inode.write_at(entry_offset.unwrap(), last_dirent.as_bytes(), &self.block_device);
            }
            root_inode.size-=DIRENT_SZ as u32;
        });
        drop(fs);
        // 删除
        if nlink_is_zero {
            let target_inode_obj=Arc::new(Self::new(
                target_block_id,
                target_block_offset,
                self.fs.clone(),
                self.block_device.clone(),
                inode_id,
            ));
            target_inode_obj.clear();
            let mut fs=self.fs.lock();
            fs.dealloc_inode(inode_id);
            drop(fs);
        }
        0
    }
    
}
