export type YarocEvent =
  | { status: 'initialized' }
  | { type: 'CellularLog'; payload: CellularLogPayload }
  | { type: 'SiPunches'; payload: Array<SiPunchLog> }
  | { type: 'SiPunch'; payload: SiPunch }
  | { type: 'MeshtasticLog'; payload: MeshtasticLogPayload }
  | { type: 'NodeInfos'; payload: Array<NodeInfo> };
