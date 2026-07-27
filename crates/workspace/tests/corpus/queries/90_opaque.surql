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
