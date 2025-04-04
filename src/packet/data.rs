// src/packet/data.rs
use crate::dpdk::ffi::RteMbuf;

/// Структура для хранения данных пакета
#[repr(C, align(64))]
pub struct PacketData {
    pub data_ptr: *const u8,
    pub data_len: usize,

    pub source_port: u16,
    pub dest_port: u16,
    pub queue_id: u16,
    pub _padding: u16,

    pub source_ip_ptr: *const u8,
    pub source_ip_len: usize,
    pub dest_ip_ptr: *const u8,
    pub dest_ip_len: usize,
    pub mbuf_ptr: *mut RteMbuf,
}

impl PacketData {
    pub fn new() -> Self {
        Self {
            data_ptr: std::ptr::null(),
            data_len: 0,

            source_port: 0,
            dest_port: 0,
            queue_id: 0,
            _padding: 0,

            source_ip_ptr: std::ptr::null(),
            source_ip_len: 0,
            dest_ip_ptr: std::ptr::null(),
            dest_ip_len: 0,
            mbuf_ptr: std::ptr::null_mut(),
        }
    }

    /// Сбрасывает все поля в исходное состояние
    /// Используется при возврате в пул
    #[inline(always)]
    pub fn reset(&mut self) {
        self.data_ptr = std::ptr::null();
        self.data_len = 0;

        self.source_port = 0;
        self.dest_port = 0;
        self.queue_id = 0;

        self.source_ip_ptr = std::ptr::null();
        self.source_ip_len = 0;
        self.dest_ip_ptr = std::ptr::null();
        self.dest_ip_len = 0;
        self.mbuf_ptr = std::ptr::null_mut();
    }

    /// Получает исходный IP-адрес в виде среза
    #[inline(always)]
    pub fn get_source_ip(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.source_ip_ptr, self.source_ip_len) }
    }

    /// Получает IP-адрес назначения в виде среза
    #[inline(always)]
    pub fn get_dest_ip(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.dest_ip_ptr, self.dest_ip_len) }
    }

    /// Получает данные пакета в виде среза
    #[inline(always)]
    pub fn get_data(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.data_ptr, self.data_len) }
    }
}

unsafe impl Send for PacketData {}
