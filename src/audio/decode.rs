//! 后台解码线程池：worker 线程构造 `StaticSoundData`（kira 无后台解码 API，
//! 但 `from_file` 是纯 CPU 解码，可在工作线程执行；结果送回主线程缓存）。

use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{mpsc, Arc, Condvar, Mutex},
    thread,
};

use kira::sound::static_sound::StaticSoundData;

/// 后台解码线程数。
pub(crate) const DECODE_THREADS: usize = 4;

/// 后台解码线程池：worker 线程构造 `StaticSoundData`（kira 无后台解码 API，但
/// `from_file` 是纯 CPU 解码，可在工作线程执行；结果送回主线程缓存）。
///
/// 谱面剩余音频（首批之外）在此渐进解码，避免游玩中 `play_synced` 主线程现解卡顿。
pub(crate) struct DecodePool {
    tasks: Arc<Mutex<VecDeque<PathBuf>>>,
    condvar: Arc<Condvar>,
    #[allow(dead_code)] // 持有句柄保持线程存活（进程退出时随进程结束）
    _handles: Vec<thread::JoinHandle<()>>,
}

impl DecodePool {
    /// 启动线程池（worker 阻塞等待任务队列）。
    pub(crate) fn new(tx: mpsc::Sender<(PathBuf, Option<StaticSoundData>)>) -> Self {
        let tasks: Arc<Mutex<VecDeque<PathBuf>>> = Arc::new(Mutex::new(VecDeque::new()));
        let condvar = Arc::new(Condvar::new());
        let mut handles = Vec::new();
        for _ in 0..DECODE_THREADS {
            let tasks = tasks.clone();
            let condvar = condvar.clone();
            let tx = tx.clone();
            handles.push(thread::spawn(move || loop {
                // 取一个任务（空队列时阻塞等待）
                let path = {
                    let mut q = match tasks.lock() {
                        Ok(q) => q,
                        Err(_) => return,
                    };
                    loop {
                        if let Some(p) = q.pop_front() {
                            break p;
                        }
                        // 空队列：等待唤醒（wait 原子释放锁，唤醒后重新拿锁）
                        match condvar.wait(q) {
                            Ok(q2) => q = q2,
                            Err(_) => return,
                        }
                    }
                };
                let result = StaticSoundData::from_file(&path).ok();
                if tx.send((path, result)).is_err() {
                    break; // 接收端已销毁（应用退出）
                }
            }));
        }
        Self {
            tasks,
            condvar,
            _handles: handles,
        }
    }

    /// 提交一个解码任务。
    pub(crate) fn submit(&self, path: PathBuf) {
        let mut q = match self.tasks.lock() {
            Ok(q) => q,
            Err(_) => return,
        };
        q.push_back(path);
        drop(q);
        self.condvar.notify_one();
    }
}
