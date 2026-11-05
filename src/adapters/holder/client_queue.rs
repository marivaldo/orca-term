//! Adapter: the frames waiting to be written to one attached client.
//!
//! The holder's reader never waits for a client: it puts each frame in the client's queue and
//! goes on, and the client's writer takes the frames out as fast as the client reads them. The
//! queue is limited by the bytes waiting in it, not by the number of frames: a frame is whatever
//! one read of the agent's terminal returned (about 1 KiB on macOS, up to 4 KiB on Linux), so a
//! number of frames says little about how far behind a client is.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};

/// A queue that holds at most `limit` bytes: the end the reader puts frames in, and the end the
/// client's writer takes them from.
pub(crate) fn client_queue(limit: usize) -> (ClientQueue, ClientFrames) {
    let (sender, receiver) = mpsc::channel();
    let waiting = Arc::new(AtomicUsize::new(0));
    let queue = ClientQueue {
        frames: sender,
        waiting: Arc::clone(&waiting),
        limit,
    };
    let frames = ClientFrames {
        frames: receiver,
        waiting,
    };
    (queue, frames)
}

/// Where the reader puts the frames for one client. Dropping it lets the writer take what is left
/// and then end.
pub(crate) struct ClientQueue {
    frames: Sender<Vec<u8>>,
    /// The bytes put in and not taken out yet.
    waiting: Arc<AtomicUsize>,
    limit: usize,
}

impl ClientQueue {
    /// Puts `frame` in the queue. Answers false, with the frame left out, when the client is to
    /// be dropped: its writer is gone, or the frame would not fit within the limit.
    pub(crate) fn push(&self, frame: Vec<u8>) -> bool {
        let size = frame.len();
        if self.waiting.load(Ordering::SeqCst) + size > self.limit {
            return false;
        }
        self.waiting.fetch_add(size, Ordering::SeqCst);
        self.frames.send(frame).is_ok()
    }
}

/// Where one client's writer takes its frames from.
pub(crate) struct ClientFrames {
    frames: Receiver<Vec<u8>>,
    waiting: Arc<AtomicUsize>,
}

impl ClientFrames {
    /// The next frame, waiting for one to come. `None` once the queue was dropped and is empty.
    pub(crate) fn next(&self) -> Option<Vec<u8>> {
        let frame = self.frames.recv().ok()?;
        self.waiting.fetch_sub(frame.len(), Ordering::SeqCst);
        Some(frame)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_come_out_in_the_order_they_went_in() {
        let (queue, frames) = client_queue(10);

        assert!(queue.push(b"one".to_vec()));
        assert!(queue.push(b"two".to_vec()));

        assert_eq!(frames.next().unwrap(), b"one");
        assert_eq!(frames.next().unwrap(), b"two");
    }

    #[test]
    fn a_frame_that_does_not_fit_is_refused_until_the_writer_makes_room() {
        let (queue, frames) = client_queue(10);
        assert!(queue.push(vec![0; 6]));

        assert!(!queue.push(vec![0; 5]));
        assert!(queue.push(vec![0; 4]));
        assert!(!queue.push(vec![0; 1]));

        assert_eq!(frames.next().unwrap().len(), 6);
        assert!(queue.push(vec![0; 5]));
    }

    #[test]
    fn many_small_frames_fit_as_long_as_their_bytes_do() {
        let (queue, _frames) = client_queue(4096);

        for _ in 0..4096 {
            assert!(queue.push(vec![0; 1]));
        }
        assert!(!queue.push(vec![0; 1]));
    }

    #[test]
    fn a_queue_whose_writer_is_gone_refuses_every_frame() {
        let (queue, frames) = client_queue(10);
        drop(frames);

        assert!(!queue.push(b"one".to_vec()));
    }

    #[test]
    fn a_dropped_queue_still_hands_out_what_it_held_and_then_ends() {
        let (queue, frames) = client_queue(10);
        assert!(queue.push(b"one".to_vec()));
        drop(queue);

        assert_eq!(frames.next().unwrap(), b"one");
        assert!(frames.next().is_none());
    }
}
