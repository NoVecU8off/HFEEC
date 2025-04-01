use std::mem::size_of;
use std::os::raw::{c_int, c_ulong, c_void};

#[link(name = "numa")]
extern "C" {
    pub fn numa_available() -> c_int;
    pub fn numa_max_node() -> c_int;
    pub fn numa_node_to_cpus(node: c_int, mask: *mut c_ulong, size: c_int) -> c_int;
    pub fn numa_alloc_onnode(size: usize, node: c_int) -> *mut c_void;
    pub fn numa_free(start: *mut c_void, size: usize);
    pub fn numa_run_on_node(node: c_int) -> c_int;
    pub fn numa_bind(nodemask: *const c_ulong);
    pub fn numa_set_localalloc();
    pub fn numa_alloc_local(size: usize) -> *mut c_void;
    pub fn numa_preferred() -> c_int;
}

pub struct NumaAllocator;

impl NumaAllocator {
    /// Проверяет, доступна ли NUMA в системе
    pub fn is_available() -> bool {
        unsafe { numa_available() >= 0 }
    }

    /// Возвращает количество узлов NUMA
    pub fn get_node_count() -> usize {
        if Self::is_available() {
            unsafe { (numa_max_node() + 1) as usize }
        } else {
            1 // Если NUMA не доступна, возвращаем 1 узел
        }
    }

    /// Получает список CPU, принадлежащих узлу NUMA
    pub fn get_node_cpus(node: usize) -> Vec<usize> {
        if !Self::is_available() {
            return Vec::new();
        }

        let num_possible_cpus = num_cpus::get();
        let mask_size =
            (num_possible_cpus + 8 * size_of::<c_ulong>() - 1) / (8 * size_of::<c_ulong>());
        let mut cpu_mask = vec![0 as c_ulong; mask_size];

        let result = unsafe {
            numa_node_to_cpus(
                node as c_int,
                cpu_mask.as_mut_ptr(),
                (mask_size * size_of::<c_ulong>()) as c_int,
            )
        };

        if result != 0 {
            return Vec::new();
        }

        // Преобразуем битовую маску в список CPU
        let mut cpus = Vec::new();
        for i in 0..num_possible_cpus {
            let word_index = i / (8 * size_of::<c_ulong>());
            let bit_index = i % (8 * size_of::<c_ulong>());

            if word_index < mask_size && (cpu_mask[word_index] & (1 << bit_index)) != 0 {
                cpus.push(i);
            }
        }

        cpus
    }

    /// Выделяет память на конкретном узле NUMA
    pub fn alloc_on_node(size: usize, node: usize) -> *mut c_void {
        if !Self::is_available() {
            return std::ptr::null_mut();
        }

        unsafe { numa_alloc_onnode(size, node as c_int) }
    }

    /// Освобождает память, выделенную через NUMA
    pub fn free(ptr: *mut c_void, size: usize) {
        if !ptr.is_null() {
            unsafe { numa_free(ptr, size) };
        }
    }

    /// Привязывает текущий поток к узлу NUMA
    pub fn bind_thread_to_node(node: usize) -> bool {
        if !Self::is_available() {
            return false;
        }

        unsafe { numa_run_on_node(node as c_int) == 0 }
    }

    /// Устанавливает локальное выделение памяти
    pub fn set_local_alloc() {
        if Self::is_available() {
            unsafe { numa_set_localalloc() };
        }
    }

    /// Возвращает предпочтительный узел NUMA для текущего потока
    pub fn get_preferred_node() -> Option<usize> {
        if Self::is_available() {
            let node = unsafe { numa_preferred() };
            if node >= 0 {
                return Some(node as usize);
            }
        }
        None
    }
}
