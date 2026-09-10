-- 3xxx — graph and relations. One intended violation per line. `likes` is
-- declared `IN account OUT article`; `article` is a plain table.

-- 3001: a step through a table that is not a relation.
SELECT ->article->account AS x FROM account WHERE id = account:ada;

-- 3002: a traversal against the relation's declared direction.
SELECT ->likes->account AS x FROM article WHERE id = article:hello;
RELATE article:hello->likes->account:ada SET weight = 1.0;

-- 3004: a FROM-position chain that stops on the edge.
SELECT weight FROM account->likes;

-- 3009: a traversal that starts from a non-record value.
SELECT views->likes->article AS x FROM article WHERE id = article:hello;

-- 3011: a recursion with no upper bound.
SELECT ->likes->article.{..}.title AS x FROM account WHERE id = account:ada;
