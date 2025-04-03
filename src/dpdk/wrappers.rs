// src/dpdk/wrappers.rs
use std::fmt;
use std::marker::PhantomData;

use crate::dpdk::ffi::RteMbuf;

#[repr(transparent)]
pub struct SendableMbufPtr {
    ptr: *mut RteMbuf,
    // Фантомные данные для обеспечения корректного поведения при копировании
    _phantom: PhantomData<RteMbuf>,
}

unsafe impl Send for SendableMbufPtr {}

unsafe impl Sync for SendableMbufPtr {}

impl SendableMbufPtr {
    pub fn new(ptr: *mut RteMbuf) -> Self {
        Self {
            ptr,
            _phantom: PhantomData,
        }
    }

    pub fn as_ptr(&self) -> *mut RteMbuf {
        self.ptr
    }

    pub fn is_null(&self) -> bool {
        self.ptr.is_null()
    }
}

impl Default for SendableMbufPtr {
    fn default() -> Self {
        Self {
            ptr: std::ptr::null_mut(),
            _phantom: PhantomData,
        }
    }
}

impl fmt::Debug for SendableMbufPtr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SendableMbufPtr({:p})", self.ptr)
    }
}

impl Clone for SendableMbufPtr {
    fn clone(&self) -> Self {
        Self {
            ptr: self.ptr,
            _phantom: PhantomData,
        }
    }
}

impl Copy for SendableMbufPtr {}

/// Безопасная обертка для буфера указателей на RteMbuf, реализующая Send
pub struct SendableMbufBuffer {
    ptr: *mut *mut RteMbuf,
    capacity: usize,
    _phantom: PhantomData<*mut RteMbuf>,
}

unsafe impl Send for SendableMbufBuffer {}

impl SendableMbufBuffer {
    pub fn new(capacity: usize) -> Self {
        let layout = std::alloc::Layout::array::<*mut RteMbuf>(capacity)
            .expect("Failed to create layout for mbuf buffer");
        let ptr = unsafe { std::alloc::alloc(layout) as *mut *mut RteMbuf };

        for i in 0..capacity {
            unsafe {
                *ptr.add(i) = std::ptr::null_mut();
            }
        }

        Self {
            ptr,
            capacity,
            _phantom: PhantomData,
        }
    }

    /// Возвращает указатель на буфер
    pub fn as_mut_ptr(&mut self) -> *mut *mut RteMbuf {
        self.ptr
    }

    /// Получает указатель на RteMbuf по индексу
    pub fn get(&self, index: usize) -> *mut RteMbuf {
        if index >= self.capacity {
            panic!("Index out of bounds");
        }
        unsafe { *self.ptr.add(index) }
    }

    /// Устанавливает указатель на RteMbuf по индексу
    pub fn set(&mut self, index: usize, mbuf: *mut RteMbuf) {
        if index >= self.capacity {
            panic!("Index out of bounds");
        }
        unsafe {
            *self.ptr.add(index) = mbuf;
        }
    }

    /// Возвращает вместимость буфера
    pub fn capacity(&self) -> usize {
        self.capacity
    }
}

impl Drop for SendableMbufBuffer {
    fn drop(&mut self) {
        // Освобождаем память буфера
        if !self.ptr.is_null() {
            unsafe {
                let layout = std::alloc::Layout::array::<*mut RteMbuf>(self.capacity)
                    .expect("Failed to create layout for mbuf buffer");
                std::alloc::dealloc(self.ptr as *mut u8, layout);
            }
        }
    }
}
