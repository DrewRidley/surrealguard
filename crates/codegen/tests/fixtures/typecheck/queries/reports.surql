-- A standalone query file. It is analyzed against the schema like any other
-- source, but only host-embedded queries reach the generated registry, so
-- nothing here appears in the golden output.

SELECT name, age FROM person WHERE age > 30 ORDER BY age DESC;
