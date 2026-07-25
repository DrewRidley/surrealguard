-- Reads of the deliberately-unknowable shapes. These sites are `any` on
-- purpose and are frozen with a reason in the ratchet baseline.

SELECT provider, payload FROM webhook_event;

LET $payload = (SELECT VALUE payload FROM ONLY webhook_event LIMIT 1);

LET $headers = (SELECT VALUE headers FROM ONLY webhook_event LIMIT 1);

RETURN { payload: $payload, headers: $headers };
