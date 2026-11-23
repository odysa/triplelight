// Derived from oxrdf 0.3.5 (MIT); see THIRD_PARTY_NOTICES.md.

pub mod rdf {
    use crate::named_node::NamedNodeRef;

    pub const ALT: NamedNodeRef<'_> =
        NamedNodeRef::new_unchecked("http://www.w3.org/1999/02/22-rdf-syntax-ns#Alt");
    pub const BAG: NamedNodeRef<'_> =
        NamedNodeRef::new_unchecked("http://www.w3.org/1999/02/22-rdf-syntax-ns#Bag");
    #[cfg(feature = "rdf-12")]
    pub const DIR_LANG_STRING: NamedNodeRef<'_> =
        NamedNodeRef::new_unchecked("http://www.w3.org/1999/02/22-rdf-syntax-ns#dirLangString");
    pub const FIRST: NamedNodeRef<'_> =
        NamedNodeRef::new_unchecked("http://www.w3.org/1999/02/22-rdf-syntax-ns#first");
    pub const HTML: NamedNodeRef<'_> =
        NamedNodeRef::new_unchecked("http://www.w3.org/1999/02/22-rdf-syntax-ns#HTML");
    #[cfg(feature = "rdf-12")]
    pub const JSON: NamedNodeRef<'_> =
        NamedNodeRef::new_unchecked("http://www.w3.org/1999/02/22-rdf-syntax-ns#JSON");
    pub const LANG_STRING: NamedNodeRef<'_> =
        NamedNodeRef::new_unchecked("http://www.w3.org/1999/02/22-rdf-syntax-ns#langString");
    pub const LIST: NamedNodeRef<'_> =
        NamedNodeRef::new_unchecked("http://www.w3.org/1999/02/22-rdf-syntax-ns#List");
    pub const NIL: NamedNodeRef<'_> =
        NamedNodeRef::new_unchecked("http://www.w3.org/1999/02/22-rdf-syntax-ns#nil");
    pub const OBJECT: NamedNodeRef<'_> =
        NamedNodeRef::new_unchecked("http://www.w3.org/1999/02/22-rdf-syntax-ns#object");
    pub const PREDICATE: NamedNodeRef<'_> =
        NamedNodeRef::new_unchecked("http://www.w3.org/1999/02/22-rdf-syntax-ns#predicate");
    pub const PROPERTY: NamedNodeRef<'_> =
        NamedNodeRef::new_unchecked("http://www.w3.org/1999/02/22-rdf-syntax-ns#Property");
    #[cfg(feature = "rdf-12")]
    pub const REIFIES: NamedNodeRef<'_> =
        NamedNodeRef::new_unchecked("http://www.w3.org/1999/02/22-rdf-syntax-ns#reifies");
    pub const REST: NamedNodeRef<'_> =
        NamedNodeRef::new_unchecked("http://www.w3.org/1999/02/22-rdf-syntax-ns#rest");
    pub const SEQ: NamedNodeRef<'_> =
        NamedNodeRef::new_unchecked("http://www.w3.org/1999/02/22-rdf-syntax-ns#Seq");
    pub const STATEMENT: NamedNodeRef<'_> =
        NamedNodeRef::new_unchecked("http://www.w3.org/1999/02/22-rdf-syntax-ns#Statement");
    pub const SUBJECT: NamedNodeRef<'_> =
        NamedNodeRef::new_unchecked("http://www.w3.org/1999/02/22-rdf-syntax-ns#subject");
    pub const TYPE: NamedNodeRef<'_> =
        NamedNodeRef::new_unchecked("http://www.w3.org/1999/02/22-rdf-syntax-ns#type");
    pub const VALUE: NamedNodeRef<'_> =
        NamedNodeRef::new_unchecked("http://www.w3.org/1999/02/22-rdf-syntax-ns#value");
    pub const XML_LITERAL: NamedNodeRef<'_> =
        NamedNodeRef::new_unchecked("http://www.w3.org/1999/02/22-rdf-syntax-ns#XMLLiteral");
}

pub mod rdfs {
    use crate::named_node::NamedNodeRef;

    pub const CLASS: NamedNodeRef<'_> =
        NamedNodeRef::new_unchecked("http://www.w3.org/2000/01/rdf-schema#Class");
    pub const COMMENT: NamedNodeRef<'_> =
        NamedNodeRef::new_unchecked("http://www.w3.org/2000/01/rdf-schema#comment");
    pub const CONTAINER: NamedNodeRef<'_> =
        NamedNodeRef::new_unchecked("http://www.w3.org/2000/01/rdf-schema#Container");
    pub const CONTAINER_MEMBERSHIP_PROPERTY: NamedNodeRef<'_> = NamedNodeRef::new_unchecked(
        "http://www.w3.org/2000/01/rdf-schema#ContainerMembershipProperty",
    );
    pub const DATATYPE: NamedNodeRef<'_> =
        NamedNodeRef::new_unchecked("http://www.w3.org/2000/01/rdf-schema#Datatype");
    pub const DOMAIN: NamedNodeRef<'_> =
        NamedNodeRef::new_unchecked("http://www.w3.org/2000/01/rdf-schema#domain");
    pub const IS_DEFINED_BY: NamedNodeRef<'_> =
        NamedNodeRef::new_unchecked("http://www.w3.org/2000/01/rdf-schema#isDefinedBy");
    pub const LABEL: NamedNodeRef<'_> =
        NamedNodeRef::new_unchecked("http://www.w3.org/2000/01/rdf-schema#label");
    pub const LITERAL: NamedNodeRef<'_> =
        NamedNodeRef::new_unchecked("http://www.w3.org/2000/01/rdf-schema#Literal");
    pub const MEMBER: NamedNodeRef<'_> =
        NamedNodeRef::new_unchecked("http://www.w3.org/2000/01/rdf-schema#member");
    pub const RANGE: NamedNodeRef<'_> =
        NamedNodeRef::new_unchecked("http://www.w3.org/2000/01/rdf-schema#range");
    pub const RESOURCE: NamedNodeRef<'_> =
        NamedNodeRef::new_unchecked("http://www.w3.org/2000/01/rdf-schema#Resource");
    pub const SEE_ALSO: NamedNodeRef<'_> =
        NamedNodeRef::new_unchecked("http://www.w3.org/2000/01/rdf-schema#seeAlso");
    pub const SUB_CLASS_OF: NamedNodeRef<'_> =
        NamedNodeRef::new_unchecked("http://www.w3.org/2000/01/rdf-schema#subClassOf");
    pub const SUB_PROPERTY_OF: NamedNodeRef<'_> =
        NamedNodeRef::new_unchecked("http://www.w3.org/2000/01/rdf-schema#subPropertyOf");
}

pub mod xsd {
    use crate::named_node::NamedNodeRef;

    pub const ANY_URI: NamedNodeRef<'_> =
        NamedNodeRef::new_unchecked("http://www.w3.org/2001/XMLSchema#anyURI");
    pub const BASE_64_BINARY: NamedNodeRef<'_> =
        NamedNodeRef::new_unchecked("http://www.w3.org/2001/XMLSchema#base64Binary");
    pub const BOOLEAN: NamedNodeRef<'_> =
        NamedNodeRef::new_unchecked("http://www.w3.org/2001/XMLSchema#boolean");
    pub const BYTE: NamedNodeRef<'_> =
        NamedNodeRef::new_unchecked("http://www.w3.org/2001/XMLSchema#byte");
    pub const DATE: NamedNodeRef<'_> =
        NamedNodeRef::new_unchecked("http://www.w3.org/2001/XMLSchema#date");
    pub const DAY_TIME_DURATION: NamedNodeRef<'_> =
        NamedNodeRef::new_unchecked("http://www.w3.org/2001/XMLSchema#dayTimeDuration");
    pub const DATE_TIME: NamedNodeRef<'_> =
        NamedNodeRef::new_unchecked("http://www.w3.org/2001/XMLSchema#dateTime");
    pub const DATE_TIME_STAMP: NamedNodeRef<'_> =
        NamedNodeRef::new_unchecked("http://www.w3.org/2001/XMLSchema#dateTimeStamp");
    pub const DECIMAL: NamedNodeRef<'_> =
        NamedNodeRef::new_unchecked("http://www.w3.org/2001/XMLSchema#decimal");
    pub const DOUBLE: NamedNodeRef<'_> =
        NamedNodeRef::new_unchecked("http://www.w3.org/2001/XMLSchema#double");
    pub const DURATION: NamedNodeRef<'_> =
        NamedNodeRef::new_unchecked("http://www.w3.org/2001/XMLSchema#duration");
    pub const FLOAT: NamedNodeRef<'_> =
        NamedNodeRef::new_unchecked("http://www.w3.org/2001/XMLSchema#float");
    pub const G_DAY: NamedNodeRef<'_> =
        NamedNodeRef::new_unchecked("http://www.w3.org/2001/XMLSchema#gDay");
    pub const G_MONTH: NamedNodeRef<'_> =
        NamedNodeRef::new_unchecked("http://www.w3.org/2001/XMLSchema#gMonth");
    pub const G_MONTH_DAY: NamedNodeRef<'_> =
        NamedNodeRef::new_unchecked("http://www.w3.org/2001/XMLSchema#gMonthDay");
    pub const G_YEAR: NamedNodeRef<'_> =
        NamedNodeRef::new_unchecked("http://www.w3.org/2001/XMLSchema#gYear");
    pub const G_YEAR_MONTH: NamedNodeRef<'_> =
        NamedNodeRef::new_unchecked("http://www.w3.org/2001/XMLSchema#gYearMonth");
    pub const HEX_BINARY: NamedNodeRef<'_> =
        NamedNodeRef::new_unchecked("http://www.w3.org/2001/XMLSchema#hexBinary");
    pub const INT: NamedNodeRef<'_> =
        NamedNodeRef::new_unchecked("http://www.w3.org/2001/XMLSchema#int");
    pub const INTEGER: NamedNodeRef<'_> =
        NamedNodeRef::new_unchecked("http://www.w3.org/2001/XMLSchema#integer");
    pub const LANGUAGE: NamedNodeRef<'_> =
        NamedNodeRef::new_unchecked("http://www.w3.org/2001/XMLSchema#language");
    pub const LONG: NamedNodeRef<'_> =
        NamedNodeRef::new_unchecked("http://www.w3.org/2001/XMLSchema#long");
    pub const NAME: NamedNodeRef<'_> =
        NamedNodeRef::new_unchecked("http://www.w3.org/2001/XMLSchema#Name");
    pub const NC_NAME: NamedNodeRef<'_> =
        NamedNodeRef::new_unchecked("http://www.w3.org/2001/XMLSchema#NCName");
    pub const NEGATIVE_INTEGER: NamedNodeRef<'_> =
        NamedNodeRef::new_unchecked("http://www.w3.org/2001/XMLSchema#negativeInteger");
    pub const NMTOKEN: NamedNodeRef<'_> =
        NamedNodeRef::new_unchecked("http://www.w3.org/2001/XMLSchema#NMTOKEN");
    pub const NON_NEGATIVE_INTEGER: NamedNodeRef<'_> =
        NamedNodeRef::new_unchecked("http://www.w3.org/2001/XMLSchema#nonNegativeInteger");
    pub const NON_POSITIVE_INTEGER: NamedNodeRef<'_> =
        NamedNodeRef::new_unchecked("http://www.w3.org/2001/XMLSchema#nonPositiveInteger");
    pub const NORMALIZED_STRING: NamedNodeRef<'_> =
        NamedNodeRef::new_unchecked("http://www.w3.org/2001/XMLSchema#normalizedString");
    pub const POSITIVE_INTEGER: NamedNodeRef<'_> =
        NamedNodeRef::new_unchecked("http://www.w3.org/2001/XMLSchema#positiveInteger");
    pub const TIME: NamedNodeRef<'_> =
        NamedNodeRef::new_unchecked("http://www.w3.org/2001/XMLSchema#time");
    pub const SHORT: NamedNodeRef<'_> =
        NamedNodeRef::new_unchecked("http://www.w3.org/2001/XMLSchema#short");
    pub const STRING: NamedNodeRef<'_> =
        NamedNodeRef::new_unchecked("http://www.w3.org/2001/XMLSchema#string");
    pub const TOKEN: NamedNodeRef<'_> =
        NamedNodeRef::new_unchecked("http://www.w3.org/2001/XMLSchema#token");
    pub const UNSIGNED_BYTE: NamedNodeRef<'_> =
        NamedNodeRef::new_unchecked("http://www.w3.org/2001/XMLSchema#unsignedByte");
    pub const UNSIGNED_INT: NamedNodeRef<'_> =
        NamedNodeRef::new_unchecked("http://www.w3.org/2001/XMLSchema#unsignedInt");
    pub const UNSIGNED_LONG: NamedNodeRef<'_> =
        NamedNodeRef::new_unchecked("http://www.w3.org/2001/XMLSchema#unsignedLong");
    pub const UNSIGNED_SHORT: NamedNodeRef<'_> =
        NamedNodeRef::new_unchecked("http://www.w3.org/2001/XMLSchema#unsignedShort");
    pub const YEAR_MONTH_DURATION: NamedNodeRef<'_> =
        NamedNodeRef::new_unchecked("http://www.w3.org/2001/XMLSchema#yearMonthDuration");
}

pub mod geosparql {
    use crate::named_node::NamedNodeRef;

    pub const WKT_LITERAL: NamedNodeRef<'_> =
        NamedNodeRef::new_unchecked("http://www.opengis.net/ont/geosparql#wktLiteral");
}

