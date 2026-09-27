use std::collections::HashMap;
use std::time::{Duration, Instant};

const MIN_RTO: Duration = Duration::from_millis(500);
const MAX_RTO: Duration = Duration::from_secs(15);
const INITIAL_RTO: Duration = Duration::from_millis(1500);

pub const MAX_RETRIES: u32 = 5;

pub struct InFlightChunk {
    pub chunk_index: u32,
    pub sent_at: Instant,
    pub retries: u32,
}

pub struct SlidingWindow {
    pub window_size: usize,
    pub base_chunk: u32,
    pub total_chunks: u32,
    pub in_flight: HashMap<u32, InFlightChunk>,
    pub acked: HashMap<u32, bool>,

    // Jacobson/Karels RTO estimator
    pub srtt: Duration,
    pub rttvar: Duration,
    pub rto: Duration,
}

impl SlidingWindow {
    pub fn new(total_chunks: u32, window_size: usize) -> Self {
        Self {
            window_size,
            base_chunk: 0,
            total_chunks,
            in_flight: HashMap::new(),
            acked: HashMap::new(),
            srtt: Duration::from_millis(500),
            rttvar: Duration::from_millis(250),
            rto: INITIAL_RTO,
        }
    }

    /// Checks whether any chunk in-flight has exceeded MAX_RETRIES (N5).
    pub fn has_exceeded_max_retries(&self) -> bool {
        self.in_flight.values().any(|entry| entry.retries >= MAX_RETRIES)
    }

    /// Determines whether the next chunk can be sent according to sliding window size.
    pub fn can_send(&self, chunk_index: u32) -> bool {
        chunk_index < self.total_chunks
            && (chunk_index as usize) < (self.base_chunk as usize + self.window_size)
            && !self.acked.contains_key(&chunk_index)
            && !self.in_flight.contains_key(&chunk_index)
    }

    /// Registers a chunk as sent into flight.
    pub fn on_chunk_sent(&mut self, chunk_index: u32) {
        self.in_flight.insert(
            chunk_index,
            InFlightChunk {
                chunk_index,
                sent_at: Instant::now(),
                retries: 0,
            },
        );
    }

    /// Handles an ACK frame from receiver, updating RTT and sliding window.
    pub fn on_ack(&mut self, chunk_index: u32) {
        if let Some(in_flight) = self.in_flight.remove(&chunk_index) {
            if in_flight.retries == 0 {
                // Update RTT estimation if not retransmitted (Karn's algorithm)
                self.update_rtt(in_flight.sent_at.elapsed());
            }
        }

        self.acked.insert(chunk_index, true);

        // Slide window left edge
        while self.base_chunk < self.total_chunks && self.acked.contains_key(&self.base_chunk) {
            self.base_chunk += 1;
        }
    }

    /// Handles a NACK frame, requesting immediate fast retransmission (N1 / N5).
    pub fn on_nack(&mut self, chunk_index: u32) -> Option<u32> {
        if let Some(entry) = self.in_flight.get_mut(&chunk_index) {
            entry.retries += 1;
            entry.sent_at = Instant::now();
            if entry.retries > MAX_RETRIES {
                return None;
            }
            return Some(chunk_index);
        }
        None
    }

    /// Checks for timed-out in-flight chunks and returns list of chunks needing retransmission.
    pub fn check_timeouts(&mut self, now: Instant) -> Vec<u32> {
        let mut to_retransmit = Vec::new();
        for (idx, in_flight) in self.in_flight.iter_mut() {
            if now.duration_since(in_flight.sent_at) > self.rto {
                in_flight.retries += 1;
                in_flight.sent_at = now;
                if in_flight.retries <= MAX_RETRIES {
                    to_retransmit.push(*idx);
                }
            }
        }

        if !to_retransmit.is_empty() {
            // Exponential backoff
            self.rto = (self.rto * 3 / 2).min(MAX_RTO);
        }

        to_retransmit
    }

    /// Jacobson/Karels algorithm for updating SRTT, RTTVAR, and RTO.
    fn update_rtt(&mut self, sample: Duration) {
        let sample_ms = sample.as_millis() as f64;
        let srtt_ms = self.srtt.as_millis() as f64;
        let rttvar_ms = self.rttvar.as_millis() as f64;

        // alpha = 0.125, beta = 0.25
        let diff = (srtt_ms - sample_ms).abs();
        let new_rttvar = 0.75 * rttvar_ms + 0.25 * diff;
        let new_srtt = 0.875 * srtt_ms + 0.125 * sample_ms;
        let new_rto = new_srtt + 4.0 * new_rttvar;

        self.srtt = Duration::from_millis(new_srtt as u64);
        self.rttvar = Duration::from_millis(new_rttvar as u64);
        self.rto = Duration::from_millis(new_rto as u64).clamp(MIN_RTO, MAX_RTO);
    }

    /// Returns true if all chunks have been received and verified.
    pub fn is_complete(&self) -> bool {
        self.base_chunk >= self.total_chunks
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sliding_window_progression() {
        let mut sw = SlidingWindow::new(5, 2);
        assert!(sw.can_send(0));
        sw.on_chunk_sent(0);

        assert!(sw.can_send(1));
        sw.on_chunk_sent(1);

        // Window full (window_size=2)
        assert!(!sw.can_send(2));

        // ACK chunk 0 -> slides base
        sw.on_ack(0);
        assert_eq!(sw.base_chunk, 1);
        assert!(sw.can_send(2));
        sw.on_chunk_sent(2);

        // Out of order ACK 2
        sw.on_ack(2);
        assert_eq!(sw.base_chunk, 1); // Still waiting for 1

        // ACK 1 -> jumps base to 3
        sw.on_ack(1);
        assert_eq!(sw.base_chunk, 3);
    }

    #[test]
    fn test_sliding_window_nack_and_timeout() {
        let mut sw = SlidingWindow::new(3, 2);
        sw.on_chunk_sent(0);

        // NACK triggers retransmit
        let nack_ret = sw.on_nack(0);
        assert_eq!(nack_ret, Some(0));

        // Check timeout
        let future = Instant::now() + Duration::from_secs(20);
        let timeouts = sw.check_timeouts(future);
        assert_eq!(timeouts, vec![0]);
    }

    #[test]
    fn test_sliding_window_max_retries() {
        let mut sw = SlidingWindow::new(2, 2);
        sw.on_chunk_sent(0);

        for _ in 0..MAX_RETRIES {
            assert!(!sw.has_exceeded_max_retries());
            sw.on_nack(0);
        }

        // Exceeded MAX_RETRIES
        assert!(sw.has_exceeded_max_retries());
        assert_eq!(sw.on_nack(0), None);
    }
}
