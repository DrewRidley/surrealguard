-- Organization side of the corpus: a nested object field, a refined
-- `array<object>` (exercises `[*]` projection), `array<record<…>>` links, and
-- `COMPUTED <~relation` reverse-reference fields.

DEFINE TABLE sector SCHEMAFULL;

DEFINE FIELD name ON sector TYPE string;

DEFINE TABLE organization SCHEMAFULL;

DEFINE FIELD name ON organization TYPE string;

DEFINE FIELD legal_name ON organization TYPE string;

DEFINE FIELD logo ON organization TYPE option<string>;

DEFINE FIELD tier ON organization TYPE 'free' | 'pro' | 'enterprise' DEFAULT 'free';

DEFINE FIELD parent ON organization TYPE option<record<organization>> REFERENCE;

DEFINE FIELD owner ON organization TYPE record<account>;

DEFINE FIELD sectors ON organization TYPE array<record<sector>> DEFAULT [];

DEFINE FIELD contacts ON organization TYPE array<object> DEFAULT [];

DEFINE FIELD contacts[*] ON organization TYPE {
	label: string,
	email: string,
	primary: bool
};

DEFINE FIELD address ON organization TYPE {
	line1: string,
	line2: option<string>,
	city: string,
	country: string
};

DEFINE FIELD employees ON organization COMPUTED <~employee_of;

DEFINE FIELD subsidiaries ON organization COMPUTED <~organization;

DEFINE FIELD created_at ON organization TYPE datetime DEFAULT time::now();

DEFINE TABLE organization_unit SCHEMAFULL;

DEFINE FIELD name ON organization_unit TYPE string;

DEFINE FIELD organization ON organization_unit TYPE record<organization>;

DEFINE FIELD parent ON organization_unit TYPE option<record<organization_unit>>;

DEFINE FIELD headcount ON organization_unit TYPE int DEFAULT 0;

DEFINE TABLE organization_settings SCHEMAFULL;

DEFINE FIELD organization ON organization_settings TYPE record<organization>;

DEFINE FIELD security ON organization_settings TYPE {
	byok: {
		mode: 'lazy' | 'immediate',
		status: 'not_started' | 'running' | 'completed' | 'failed',
		progress: int,
		kms_provider: option<string>
	}
};

DEFINE FIELD updated_at ON organization_settings TYPE datetime VALUE time::now();
