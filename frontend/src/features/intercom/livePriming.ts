import { playVideo } from './hlsPlayback'

interface LiveFrame {
  url: string
  capturedAt: number
}

const freshFrameMs = 120_000
const frames = new Map<string, LiveFrame>()
let primedVideo: HTMLVideoElement | null = null

export function liveFrameKey(flatId: string, cameraId: string): string {
  return `${flatId}\u0000${cameraId}`
}

export function createLiveVideo(): HTMLVideoElement {
  const video = document.createElement('video')
  video.playsInline = true
  return video
}

// A play() call inside a click handler lets the same element play with sound later, in every autoplay policy.
export function primeVideo(video: HTMLVideoElement) {
  void playVideo(video).catch(() => undefined)
}

export function primeLiveAudio() {
  const video = primedVideo ?? createLiveVideo()
  video.muted = false
  primedVideo = video
  primeVideo(video)
}

export function takePrimedVideo(): HTMLVideoElement | null {
  const video = primedVideo
  primedVideo = null
  return video
}

export function captureLiveFrame(video: HTMLVideoElement, key: string): string | null {
  if (!video.videoWidth || !video.videoHeight) return null
  try {
    const canvas = document.createElement('canvas')
    canvas.width = video.videoWidth
    canvas.height = video.videoHeight
    const context = canvas.getContext('2d')
    if (!context) return null
    context.drawImage(video, 0, 0, canvas.width, canvas.height)
    const url = canvas.toDataURL('image/jpeg', 0.82)
    frames.set(key, { url, capturedAt: Date.now() })
    return url
  } catch {
    return null
  }
}

export function freshLiveFrame(key: string): string | null {
  const frame = frames.get(key)
  if (!frame) return null
  if (Date.now() - frame.capturedAt < freshFrameMs) return frame.url
  frames.delete(key)
  return null
}

export function clearLiveFrames() {
  frames.clear()
}
