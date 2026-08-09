/**
 * Queries for the component type tests, with a registry augmentation standing
 * in for a generated file — the same trick `test/queries.ts` uses, so the
 * component tests exercise the real typed path rather than `.unchecked`.
 *
 * The texts are distinct from every other augmentation in this package: the
 * registry is one global interface, and two files declaring the same key would
 * collide.
 */
import { defineLive, defineQuery, type RecordId } from "@surrealguard/client";

declare module "@surrealguard/client" {
  interface SurqlRegistry {
    "SELECT id, name, age FROM member": {
      result: [Array<{ id: RecordId<"member">; name: string; age: number }>];
      params: Record<string, never>;
    };
    "SELECT id, name FROM member WHERE team = $team": {
      result: [Array<{ id: RecordId<"member">; name: string }>];
      params: { team: string };
    };
    "SELECT id, name FROM member WHERE id = $id": {
      result: [Array<{ id: RecordId<"member">; name: string }>];
      params: { id: RecordId<"member"> };
    };
    "RETURN count(SELECT id FROM member)": {
      result: [number];
      params: Record<string, never>;
    };
  }
}

/** The JSON projection of a member row — what survives `load` and hydration. */
export interface MemberJson {
  id: `member:${string}`;
  name: string;
  age: number;
}

export const allMembers = defineQuery("SELECT id, name, age FROM member");
export const membersOf = defineQuery("SELECT id, name FROM member WHERE team = $team");
export const memberCount = defineQuery("RETURN count(SELECT id FROM member)");

export const liveMembers = defineLive("SELECT id, name, age FROM member");
export const liveTeam = defineLive("SELECT id, name FROM member WHERE team = $team");
export const liveMember = defineLive("SELECT id, name FROM member WHERE id = $id");
