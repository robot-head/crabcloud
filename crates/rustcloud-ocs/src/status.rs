//! Nextcloud OCS status codes. Hand-mapped to match upstream behavior so
//! existing clients see the numbers they expect.
//!
//! See spec §9.3.

/// OCS-level status (the `<statuscode>` in the envelope). Distinct from the
/// HTTP status — both are emitted, the OCS one inside the envelope, the
/// HTTP one in the wire-level response status line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OcsStatus {
    Ok,           // 100 in v1, 200 in v2
    Created,      // 201 (v2 only — rare; map to Ok in v1)
    BadRequest,   // 400
    Unauthorized, // 997 — yes, really
    Forbidden,    // 403
    NotFound,     // 998
    UnknownError, // 999
    ServerError,  // 996
}

impl OcsStatus {
    pub fn v1_code(self) -> u16 {
        match self {
            OcsStatus::Ok => 100,
            OcsStatus::Created => 100,
            OcsStatus::BadRequest => 400,
            OcsStatus::Unauthorized => 997,
            OcsStatus::Forbidden => 403,
            OcsStatus::NotFound => 998,
            OcsStatus::UnknownError => 999,
            OcsStatus::ServerError => 996,
        }
    }

    pub fn v2_code(self) -> u16 {
        match self {
            OcsStatus::Ok => 200,
            OcsStatus::Created => 201,
            OcsStatus::BadRequest => 400,
            OcsStatus::Unauthorized => 997,
            OcsStatus::Forbidden => 403,
            OcsStatus::NotFound => 998,
            OcsStatus::UnknownError => 999,
            OcsStatus::ServerError => 996,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            OcsStatus::Ok => "ok",
            OcsStatus::Created => "ok",
            OcsStatus::BadRequest => "failure",
            OcsStatus::Unauthorized => "failure",
            OcsStatus::Forbidden => "failure",
            OcsStatus::NotFound => "failure",
            OcsStatus::UnknownError => "failure",
            OcsStatus::ServerError => "failure",
        }
    }

    pub fn http_code(self) -> u16 {
        match self {
            OcsStatus::Ok => 200,
            OcsStatus::Created => 201,
            OcsStatus::BadRequest => 400,
            OcsStatus::Unauthorized => 401,
            OcsStatus::Forbidden => 403,
            OcsStatus::NotFound => 404,
            OcsStatus::UnknownError => 500,
            OcsStatus::ServerError => 500,
        }
    }
}

/// Which OCS protocol version the response is wrapped in. Affects only
/// the `statuscode` mapping (100 vs 200 for OK).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OcsVersion {
    V1,
    V2,
}

impl OcsStatus {
    pub fn code_for(self, version: OcsVersion) -> u16 {
        match version {
            OcsVersion::V1 => self.v1_code(),
            OcsVersion::V2 => self.v2_code(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ok_maps_to_100_in_v1_and_200_in_v2() {
        assert_eq!(OcsStatus::Ok.code_for(OcsVersion::V1), 100);
        assert_eq!(OcsStatus::Ok.code_for(OcsVersion::V2), 200);
    }

    #[test]
    fn nextcloud_specific_failure_codes_match_upstream() {
        assert_eq!(OcsStatus::Unauthorized.v2_code(), 997);
        assert_eq!(OcsStatus::NotFound.v2_code(), 998);
        assert_eq!(OcsStatus::UnknownError.v2_code(), 999);
        assert_eq!(OcsStatus::ServerError.v2_code(), 996);
    }

    #[test]
    fn http_codes_independent_from_ocs_codes() {
        assert_eq!(OcsStatus::Unauthorized.http_code(), 401);
        assert_eq!(OcsStatus::NotFound.http_code(), 404);
    }
}
