export interface LanDevice { id: string; name: string; protocol: number; code: string }
export interface LanOffer { sender: string; kind: 'text' | 'file'; name: string; size: number }
export interface LanRecord {
  id: string; direction: 'send' | 'receive'; peer: string; kind: 'text' | 'file'; name: string;
  size: number; transferred: number; status: 'waiting' | 'transferring' | 'completed' | 'failed';
  createdAt: number; path?: string; text?: string; error?: string;
}
export interface LanSnapshot {
  running: boolean; device?: LanDevice; addresses: string[]; peers: { address: string; name: string }[];
  incoming: { id: string; offer: LanOffer; address: string }[]; records: LanRecord[]; discoveryError?: string;
}
export interface LanConnection { connectionId: string; device: LanDevice }
