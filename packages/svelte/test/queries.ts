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
    "CREATE user SET name = $name": {
      result: [Array<{ id: RecordId<"user">; name: string }>];
      params: { name: string };
    };
  }
}

export const liveUsers = defineLive("SELECT * FROM user");
export const liveUsersOfTeam = defineLive("SELECT * FROM user WHERE team = $team");
export const allUsers = defineQuery("SELECT * FROM user");
export const addUser = defineQuery("CREATE user SET name = $name");
