/**
 * Phone sensing library (F1 calibration; shared with camera mapping F6/F7 and the receiver
 * wizard F9). See docs/ARCHITECTURE.md §12.1.
 *
 * - `camera.ts`  — getUserMedia with friendly errors, frame callbacks with capture timestamps,
 *                  exposure lock (`openMedia`, `CameraCapture`).
 * - `frames.ts`  — 160×120 luma frames, analysed in a worker (`FrameAnalyzer`, `grabLuma`).
 * - `video-onset.ts` — flash metric and sub-frame flash onsets.
 * - `mic.ts`, `audio-onset.ts` — raw microphone with phone-clock timing; chirp detector.
 * - `associate.ts`, `calibrate.ts` — schedule correlation and the calibration result.
 * - `clap.ts`, `device.ts` — per-phone timing bias.
 * - `session.ts` — everything together for the calibrate page.
 * - `components/ui/SecureGate.svelte` — "needs a secure connection" gate for such pages.
 */
export * from './camera';
export * from './frames';
export * from './video-onset';
export * from './schedule';
export * from './calibrate';
export * from './associate';
export * from './clap';
export * from './device';
