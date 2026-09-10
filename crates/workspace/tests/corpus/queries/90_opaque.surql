-- Reads of the deliberately-unknowable shapes. These sites are `any` on
-- purpose and are frozen with a reason in the ratchet baseline.

SELECT provider, payload FROM webhook_event;

LET $payload = (SELECT VALUE payload FROM ONLY webhook_event LIMIT 1);

LET $headers = (SELECT VALUE headers FROM ONLY webhook_event LIMIT 1);

RETURN { payload: $payload, headers: $headers };

-- `.*` on a link whose table declares no fields: there is no row shape to
-- expand to. The site stays `any` and SAYS so (7008) rather than falling back
-- to the link kind — after a `.*` the value is the row, and calling it a link
-- would be a lie rather than an absence.
SELECT external.* AS external_row FROM webhook_event;

-- A subpath of an open object is not a missing field: `headers` is FLEXIBLE
-- TYPE object, so `headers.trace_id` may well be there and nobody can say
-- otherwise. A condition used to claim `webhook_event` had no field
-- `headers.trace_id` — the projection has always been silent here, and now the
-- condition resolves the path with the same checker.
SELECT provider FROM webhook_event WHERE headers.trace_id != NONE;
