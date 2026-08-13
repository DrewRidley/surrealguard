// The one place the test's query text lives — exactly as an app would do it,
// with a registry augmentation standing in for the generated file. Using the
// real `defineLive`/`defineQuery` (not `.unchecked`) means the runtime tests
// exercise the typed path, so `user.name` below is a real `string`.
import { defineLive, defineQuery, type RecordId } from "@surrealguard/client";

declare module "@surrealguard/client" {
  interface SurqlRegistry {
    "SELECT * FROM user": {
      result: [Array<{ id: RecordId<"user">; name: string }>];
      params: Record<string, never>;
    };
    "SELECT * FROM user WHERE team = $team": {
      result: [Array<{ id: RecordId<"user">; name: string }>];
      params: { team: string };
    };
    // Keyed by a plain string field on purpose: the component tests nest this
    // one INSIDE a `<Query>`, parameterised by a field of the outer row, and a
    // reactive row is `Json`-shaped — so a `record` param would need
    // reconstructing from `` `user:${string}` `` before it could be passed
    // back. That cost is real (`recordId` in `@surrealguard/client` pays it),
    // but it is not what these tests are about.
    "SELECT * FROM user WHERE name = $name": {
      result: [Array<{ id: RecordId<"user">; name: string }>];
      params: { name: string };
    };
    "CREATE user SET name = $name": {
      result: [Array<{ id: RecordId<"user">; name: string }>];
      params: { name: string };
    };
  }
}

export const liveUsers = defineLive("SELECT * FROM user");
export const liveUsersOfTeam = defineLive("SELECT * FROM user WHERE team = $team");
export const liveUserNamed = defineLive("SELECT * FROM user WHERE name = $name");
export const allUsers = defineQuery("SELECT * FROM user");
export const addUser = defineQuery("CREATE user SET name = $name");
