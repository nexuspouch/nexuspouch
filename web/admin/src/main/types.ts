export type Peer = {
  fingerprint?: string;
  device_name?: string;
};

export type Stats = {
  volume_warn?: boolean;
  volume_used_ratio?: number;
  volume_free_bytes?: number;
  volume_total_bytes?: number;
  self_device?: string;
  device?: string;
  master?: string;
  master_epoch?: number;
  devices?: Record<string, Record<string, number>>;
};

export type ImportRequest = {
  request_id: string;
  new_device?: string;
  old_device?: string;
};

export type RecycleEntry = {
  space?: string;
  origin_path?: string;
  recycle_path?: string;
  size?: number;
};

export type ImportGrant = {
  grant_id?: string;
  old_device?: string;
  new_device?: string;
  spaces?: string[];
  issued_at?: number;
  expires_at?: number;
  revoked?: boolean;
};

export type BrowseEntry = {
  path?: string;
  size?: number;
};

export type MdnsPeer = {
  name?: string;
  fingerprint?: string;
  endpoint?: string;
  same_host?: boolean;
};

export type PairPending = {
  session_id: string;
  device_name?: string;
  fingerprint?: string;
};

export type PairStart = {
  qr_svg?: string;
  code?: string;
  local_endpoint?: string;
  channel_endpoint?: string;
  fingerprint?: string;
  qr?: string;
};
