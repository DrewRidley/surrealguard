-- Aggregates and GROUP. A grouped SELECT does not preserve the row shape, so
-- its response kind is the precision-sensitive part.

SELECT count() FROM organization GROUP ALL;

SELECT tier, count() AS total FROM organization GROUP BY tier;

SELECT organization, math::sum(headcount) AS heads FROM organization_unit GROUP BY organization;

SELECT math::mean(headcount) AS avg_heads FROM organization_unit GROUP ALL;

SELECT tier, array::group(name) AS names FROM organization GROUP BY tier;

SELECT status, count() AS n FROM employee_of GROUP BY status;

SELECT * FROM organization SPLIT contacts;

SELECT name, time::year(created_at) AS year FROM organization GROUP BY year;
