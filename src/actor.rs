use std::cell::{RefCell, UnsafeCell};
use std::cmp;
use std::marker::PhantomData;
use std::sync::Arc;
use std::sync::atomic::{
    AtomicBool,
    AtomicUsize,
    Ordering,
};
use std::thread;
use std::thread::JoinHandle;

use bqueue::BoundedQueue;

thread_local! {
    static CURRENT_PROCESS_ID: RefCell<Option<usize>> = RefCell::new(None);
}

type Array<T> = Box<[T]>;

enum Action {
    Dequeue,
    Other, // TODO: choose better name.
}

pub enum Error<MSG> {
    ActorIsDead,
    SendFail(MSG),
}

enum ProcessRunningState {
    Running,
    Waiting,
}

pub enum ProcessContinuation {
    Continue,
    Stop,
    WaitMessage,
}

#[repr(usize)]
enum ProcessQueueState {
    Running,
    Stopped,
}

pub struct Pid<MSG> {
    dest_processes: Arc<Array<OpaqueBox>>,
    id: usize,
    generation: AtomicUsize,
    _marker: PhantomData<MSG>,
}

impl<MSG> Clone for Pid<MSG> {
    fn clone(&self) -> Self {
        Self {
            dest_processes: self.dest_processes.clone(),
            id: self.id,
            generation: AtomicUsize::new(self.generation.load(Ordering::SeqCst)),
            _marker: PhantomData,
        }
    }
}

pub struct Process {
    id: usize,
    max_message_per_cycle: usize,
    running_state: ProcessRunningState,
    handler: Option<Box<FnMut(Action) -> Option<bool> + Send>>,
    process_pool: Arc<BoundedQueue<usize>>,
    parent: Option<usize>,
    shared_process: Arc<SharedProcess>,
}

struct SharedProcess {
    generation: AtomicUsize,
    message_queue: UnsafeCell<Option<OpaqueBox>>, // TODO: maybe remove Option?
    release_lock: AtomicBool,
}

impl Process {
    fn new(id: usize, process_pool: Arc<BoundedQueue<usize>>) -> Self {
        Self {
            id,
            max_message_per_cycle: 0,
            running_state: ProcessRunningState::Running,
            handler: None,
            process_pool,
            parent: None,
            shared_process: Arc::new(SharedProcess {
                generation: AtomicUsize::new(0),
                message_queue: UnsafeCell::new(None),
                release_lock: AtomicBool::new(false),
            }),
        }
    }

    fn reset(&mut self) {
        spin_lock(&self.shared_process.release_lock);
        self.shared_process.generation.fetch_add(1, Ordering::SeqCst);
        self.handler = None;
        unsafe {
            *self.shared_process.message_queue.get() = None;
        }
        spin_unlock(&self.shared_process.release_lock);
        while self.process_pool.push(self.id).is_err() {
        }
    }
}

pub struct ProcessQueue {
    process_capacity: usize,
    process_pool: Arc<BoundedQueue<usize>>,
    processes: UnsafeArray<OpaqueBox>,
    shared_processes: Arc<Array<OpaqueBox>>,
    shared_pq: Arc<SharedProcessQueue>,
    threads: Vec<JoinHandle<()>>,
}

impl ProcessQueue {
    pub fn new(process_capacity: usize, thread_count: usize) -> Arc<Self> {
        let mut processes = Vec::with_capacity(process_capacity);
        let mut shared_processes = Vec::with_capacity(process_capacity);
        let process_pool = Arc::new(BoundedQueue::new(process_capacity));

        for process_id in 0..process_capacity {
            let process = Process::new(process_id, Arc::clone(&process_pool));
            process_pool.push(processes.len()).expect("push to process pool");
            shared_processes.push(OpaqueBox::new(Arc::clone(&process.shared_process)));
            processes.push(OpaqueBox::new(process));
        }

        let shared_pq = Arc::new(SharedProcessQueue {
            process_count: AtomicUsize::new(0),
            run_queue: BoundedQueue::new(process_capacity),
            state: AtomicUsize::new(ProcessQueueState::Running as usize),
        });

        let mut threads = Vec::with_capacity(thread_count);

        let processes = UnsafeArray::from_vec(processes);

        for _ in 0..thread_count {
            let worker_state = WorkerState::new(Arc::clone(&shared_pq));
            let mut processes = processes.clone();
            threads.push(thread::spawn(move || {
                let queue = &worker_state.shared_pq;
                while queue.state.load(Ordering::Acquire) == ProcessQueueState::Running as usize {
                    match queue.run_queue.pop() {
                        Some(process_id) => {
                            let process = unsafe { processes.get_mut(process_id).get_mut_as::<Process>() };
                            let mut push_actor_back = true;
                            let mut msg_count = 0;
                            while msg_count < process.max_message_per_cycle && push_actor_back {
                                let handler = process.handler.as_mut().expect("process handler");
                                match process.running_state {
                                    ProcessRunningState::Running => {
                                        push_actor_back = handler(Action::Other).expect("Some boolean");
                                    },
                                    ProcessRunningState::Waiting => {
                                        match handler(Action::Dequeue) {
                                            Some(push) => push_actor_back = push,
                                            None => break,
                                        }
                                    },
                                }
                                msg_count += 1;
                            }
                            if push_actor_back {
                                while queue.run_queue.push(process_id).is_err() {
                                    thread::yield_now();
                                }
                            }
                            else {
                                // Actor died.
                                queue.process_count.fetch_sub(1, Ordering::SeqCst);
                            }
                        },
                        None => thread::yield_now(),
                    }
                }
            }));
        }

        Arc::new(Self {
            process_capacity,
            process_pool,
            processes,
            shared_processes: Arc::new(shared_processes.into_boxed_slice()),
            shared_pq: Arc::clone(&shared_pq),
            threads,
        })
    }

    fn get_pid<MSG>(&self, id: usize) -> Pid<MSG> {
        let dest_processes = self.shared_processes.clone();
        let generation = unsafe { &self.processes.get(id).get_as::<Process>().shared_process.generation };
        let generation = AtomicUsize::new(generation.load(Ordering::SeqCst));
        Pid {
            dest_processes,
            id,
            generation,
            _marker: PhantomData::<MSG>,
        }
    }

    pub fn send_message<MSG>(pid: &Pid<MSG>, msg: MSG) -> Result<(), Error<MSG>> {
        let dest_processes = &pid.dest_processes;
        let dest_process = unsafe { dest_processes[pid.id].get_as::<Arc<SharedProcess>>() };

        // We have to handle nasty situations here:
        //
        // 1. we are trying to write while the process is dying:
        //    X = genId
        //    actor dies
        //    push message
        //    actor revived
        //    new actor consumes wrong message
        //    send returns SUCCESS
        //
        // 2. we are trying to write while the process is dying:
        //    X = genId
        //    actor dies
        //    push message
        //    send returns SUCCESS, but message never processed (lesser evil)
        //
        // we need a release lock (until another better method is found)

        if spin_try_lock(&dest_process.release_lock) {
            if pid.generation.load(Ordering::SeqCst) != dest_process.generation.load(Ordering::SeqCst) {
                // TODO: switch to a lock guard?
                spin_unlock(&dest_process.release_lock);
                return Err(Error::ActorIsDead);
            }

            let message_queue = unsafe {
                (*dest_process.message_queue
                    .get()).as_ref()
                    .expect("dest_process.message_queue")
                    .get_as::<BoundedQueue<MSG>>()
            };
            match message_queue.push(msg) {
                Ok(()) => {
                    spin_unlock(&dest_process.release_lock);
                    Ok(())
                },
                Err(msg) => {
                    spin_unlock(&dest_process.release_lock);
                    Err(Error::SendFail(msg))
                },
            }
        }
        else {
            Err(Error::SendFail(msg))
        }
    }

    pub fn spawn<F, MSG>(&self, params: SpawnParameters<F>) -> Option<Pid<MSG>>
    where F: FnMut(&Pid<MSG>, Option<MSG>) -> ProcessContinuation + Send + Sync + 'static,
          MSG: Send + 'static
    {
        let process_count = self.shared_pq.process_count.fetch_add(1, Ordering::SeqCst);
        if process_count < self.process_capacity {
            let mut processes = self.processes.clone();
            let process;
            let process_id;
            let current_pid;

            loop {
                if let Some(current_process_id) = self.process_pool.pop() {
                    process_id = current_process_id;
                    current_pid = self.get_pid(process_id); // TODO: make sure it's called at the right place.
                    process = unsafe { processes.get_mut(current_process_id).get_mut_as::<Process>() };
                    break;
                }
                thread::yield_now();
            }

            let mut processes = self.processes.clone();
            let parent = CURRENT_PROCESS_ID.with(|current_process_id| {
                *current_process_id.borrow()
            });
            process.shared_process.release_lock.store(false, Ordering::SeqCst); // TODO: make sure its equivalent.
            process.parent = parent;
            process.process_pool = Arc::clone(&self.process_pool);
            let mut handler = params.handler;
            process.handler = Some(Box::new(move |action| {
                let process = unsafe { processes.get_mut(process_id).get_mut_as::<Process>() };
                match action {
                    Action::Dequeue => {
                        let message = {
                            let message_queue = unsafe {
                                (*process.shared_process.message_queue
                                    .get()).as_ref()
                                    .expect("process message queue")
                                    .get_as::<BoundedQueue<MSG>>()
                            };
                            message_queue.pop()
                        };
                        match message {
                            Some(msg) =>
                                CURRENT_PROCESS_ID.with(|current_process_id| {
                                    *current_process_id.borrow_mut() = Some(process.id);
                                    let push =
                                        match handler(&current_pid, Some(msg)) {
                                            ProcessContinuation::Stop => {
                                                process.reset();
                                                false
                                            },
                                            ProcessContinuation::WaitMessage => {
                                                process.running_state = ProcessRunningState::Waiting;
                                                true
                                            },
                                            ProcessContinuation::Continue => {
                                                process.running_state = ProcessRunningState::Running;
                                                true
                                            }
                                        };
                                    Some(push)
                                }),
                            None => None,
                        }
                    },
                    Action::Other => {
                        let push =
                            CURRENT_PROCESS_ID.with(|current_process_id| {
                                *current_process_id.borrow_mut() = Some(process.id);
                                match handler(&current_pid, None) {
                                    ProcessContinuation::Stop => {
                                        process.reset();
                                        false
                                    },
                                    ProcessContinuation::WaitMessage => {
                                        process.running_state = ProcessRunningState::Waiting;
                                        true
                                    },
                                    ProcessContinuation::Continue => {
                                        process.running_state = ProcessRunningState::Running;
                                        true
                                    },
                                }
                            });
                        Some(push)
                    },
                }
            }));
            process.running_state = ProcessRunningState::Running;
            process.max_message_per_cycle = cmp::min(params.message_capacity, params.max_message_per_cycle);
            unsafe {
                *process.shared_process.message_queue.get() = Some(OpaqueBox::new(BoundedQueue::<MSG>::new(params.message_capacity)));
            }

            while self.shared_pq.run_queue.push(process.id).is_err() {
                thread::yield_now();
            }

            Some(Pid {
                dest_processes: Arc::clone(&self.shared_processes),
                id: process.id,
                generation: AtomicUsize::new(process.shared_process.generation.load(Ordering::SeqCst)),
                _marker: PhantomData::<MSG>,
            })
        }
        else {
            self.shared_pq.process_count.fetch_sub(1, Ordering::SeqCst);
            None
        }
    }
}

impl Drop for ProcessQueue {
    fn drop(&mut self) {
        if self.shared_pq.state.load(Ordering::Acquire) == ProcessQueueState::Running as usize {
            self.shared_pq.state.store(ProcessQueueState::Stopped as usize, Ordering::Release);
            // Wait on the threads to exit.
            for thread in self.threads.drain(..) {
                thread.join().expect("thread join");
            }
        }
    }
}

struct SharedProcessQueue {
    process_count: AtomicUsize,
    // TODO: maybe change usize by a newtype?
    run_queue: BoundedQueue<usize>,
    state: AtomicUsize,
}

pub struct SpawnParameters<F> {
    pub handler: F,
    pub message_capacity: usize,
    pub max_message_per_cycle: usize,
}

struct WorkerState {
    shared_pq: Arc<SharedProcessQueue>,
}

impl WorkerState {
    fn new(shared_pq: Arc<SharedProcessQueue>) -> Self {
        Self {
            shared_pq,
        }
    }
}

fn spin_lock(lock: &AtomicBool) {
    while lock.compare_exchange_weak(false, true, Ordering::SeqCst, Ordering::SeqCst).is_err() {
    }
}

fn spin_unlock(lock: &AtomicBool) {
    while lock.compare_exchange_weak(true, false, Ordering::SeqCst, Ordering::SeqCst).is_err() {
    }
}

fn spin_try_lock(lock: &AtomicBool) -> bool {
    lock.compare_exchange_weak(false, true, Ordering::SeqCst, Ordering::SeqCst).is_ok()
}

/// Trait used for data that can be inserted inside an `OpaqueBox`.
trait Opaque { }

impl Opaque for Process { }
impl Opaque for Arc<SharedProcess> { }
impl<T> Opaque for BoundedQueue<T> { }

/// An opaque boxed type. It is useful to store heterogeneous elements inside the same collection.
/// Its use is unsafe because the user has to make sure it extract the data from the `OpaqueBox`
/// with the right type.
struct OpaqueBox {
    data: Box<Opaque>,
}

impl OpaqueBox {
    fn new<T: Opaque + 'static>(data: T) -> Self {
        Self {
            data: Box::new(data),
        }
    }

    unsafe fn get_as<T>(&self) -> &T {
        &*(&*self.data as *const _ as *const T)
    }

    unsafe fn get_mut_as<T>(&mut self) -> &mut T {
        &mut *(&*self.data as *const _ as *mut T)
    }
}

unsafe impl Send for OpaqueBox {}
unsafe impl Sync for OpaqueBox {} // TODO: check if needed.

/// Unsafe array shareable between threads.
///
/// # Unsafety
/// It is unsafe to use because there is no synchronization for the accesses, so you must make sure
/// you do not access the same element from multiple threads or that you synchronize somehow the
/// element accesses.
struct UnsafeArray<T> {
    data: Arc<[T]>,
}

impl<T> Clone for UnsafeArray<T> {
    fn clone(&self) -> Self {
        Self {
            data: Arc::clone(&self.data),
        }
    }
}

impl<T> UnsafeArray<T> {
    /// Create an `UnsafeArray` from a `Vec`.
    fn from_vec(vec: Vec<T>) -> Self {
        Self {
            data: vec.into_boxed_slice().into(),
        }
    }

    /// Get a reference to an element from the array.
    ///
    /// # Unsafety
    /// It is unsafe because another thread can mutate the specified element at the same time.
    unsafe fn get(&self, index: usize) -> &T {
        &self.data[index]
    }

    /// Get a mutable reference to an element from the array.
    ///
    /// # Unsafety
    /// It is unsafe because another thread can access the specified element at the same time.
    unsafe fn get_mut(&mut self, index: usize) -> &mut T {
        &mut *(&self.data[index] as *const _ as *mut _)
    }
}

unsafe impl<T> Send for UnsafeArray<T> {}
unsafe impl<T> Sync for UnsafeArray<T> {}
