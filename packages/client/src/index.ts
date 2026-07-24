export type {
  RecordId,
  GeoJSON,
  SurqlRegistry,
  SurqlQueryShape,
  ParamsArg,
} from "./registry.js";
export {
  SurrealGuardClient,
  type ArgsOf,
  type QueryResultOf,
} from "./client.js";
export {
  buildLiveSql,
  type LiveDescriptor,
  type LiveRowOf,
  type RowOf,
} from "./live.js";
