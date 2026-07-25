-- Flow narrowing. `FROM ONLY` yields an `option<…>`; a guard should strip the
-- `NONE`. A regression here is invisible to a diagnostics-only harness because
-- nothing becomes *wrong* — the type just stops being narrow.

LET $org = (SELECT name, tier, parent FROM ONLY organization WHERE tier = 'pro' LIMIT 1);

IF $org = NONE THEN THROW 'no organization' END;

LET $after_guard = $org.name;

LET $maybe = (SELECT VALUE logo FROM ONLY organization LIMIT 1);

LET $with_default = $maybe ?? 'no-logo';

LET $unit = (SELECT name, parent, headcount FROM ONLY organization_unit LIMIT 1);

LET $unit_name = IF $unit != NONE THEN $unit.name ELSE 'unknown' END;

LET $link = (SELECT VALUE person FROM ONLY account LIMIT 1);

LET $person_id = IF $link IS NOT NONE { RETURN $link } ELSE { RETURN NONE };

RETURN { after_guard: $after_guard, with_default: $with_default, unit_name: $unit_name };
