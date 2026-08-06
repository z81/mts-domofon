/**
 * Production boundary for a future authenticated transport. The preview never
 * instantiates this client and deliberately does not persist credentials.
 */
export type DeviceId = string & { readonly __brand: 'DeviceId' }
export type CameraId = string & { readonly __brand: 'CameraId' }

export interface IntercomDevice {
  id: DeviceId
  name: string
  online: boolean
}

export interface CameraEvent {
  id: string
  cameraId: CameraId
  occurredAt: string
  type: 'motion' | 'door_opened' | 'call'
}

export interface IntercomPreviewClient {
  listDevices(): Promise<readonly IntercomDevice[]>
  listCameraEvents(cameraId: CameraId, from: string, to: string): Promise<readonly CameraEvent[]>
  openDoor(deviceId: DeviceId): Promise<void>
}
