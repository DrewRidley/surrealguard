DEFINE TABLE team SCHEMAFULL PERMISSIONS FULL;
	DEFINE FIELD name ON team TYPE string;

DEFINE TABLE person SCHEMAFULL PERMISSIONS FULL;
	DEFINE FIELD name ON person TYPE string;
	DEFINE FIELD age ON person TYPE int;
	DEFINE FIELD team ON person TYPE record<team>;
	DEFINE FIELD email ON person TYPE option<string>;
	DEFINE FIELD password ON person TYPE option<string> PERMISSIONS NONE;

DEFINE TABLE ticket SCHEMAFULL
	PERMISSIONS
		FOR select WHERE team = $auth.team
		FOR create, update, delete NONE;
	DEFINE FIELD title ON ticket TYPE string;
	DEFINE FIELD team ON ticket TYPE record<team>;
	DEFINE FIELD done ON ticket TYPE bool DEFAULT false;

DEFINE ACCESS staff ON DATABASE TYPE RECORD
	SIGNUP (
		CREATE person SET
			name = $name,
			email = $email,
			age = $age,
			team = $team,
			password = crypto::argon2::generate($password)
	)
	SIGNIN (
		SELECT * FROM person
		WHERE email = $email AND crypto::argon2::compare(password, $password)
	)
	DURATION FOR TOKEN 15m, FOR SESSION 12h;
