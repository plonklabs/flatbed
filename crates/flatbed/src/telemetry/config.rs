use super::TelemetryError;

#[derive(Debug, Clone)]
pub struct TelemetryConfig {
    /// The name of the service using telemetry.
    pub service_name: String,
    /// The IP address of the service.
    pub ip_address: String,
    /// The telemetry server port.
    pub port: u16,
}

impl TelemetryConfig {
    /// Creates a new `TelemetryConfig` after validating the input.
    pub fn new(
        service_name: Option<String>,
        ip_address: Option<String>,
        port: Option<u16>,
    ) -> Result<Self, TelemetryError> {
        let service_name = service_name
            .or_else(|| std::env::var("FLATBED_SERVICE_NAME").ok())
            .ok_or_else(|| TelemetryError::ConfigValidationError("FLATBED_SERVICE_NAME environment variable is not set and no service name was provided".to_string()))?;

        let ip_address = ip_address
            .or_else(|| std::env::var("FLATBED_SERVICE_ADDRESS").ok())
            .ok_or_else(|| TelemetryError::ConfigValidationError("FLATBED_SERVICE_ADDRESS environment variable is not set and no service name was provided".to_string()))?;

        let port = port
            .or_else(|| {
                std::env::var("FLATBED_TELEMETRY_PORT")
                    .ok()
                    .and_then(|p| p.parse().ok())
            })
            .unwrap_or(8080);

        let config = TelemetryConfig {
            service_name,
            ip_address,
            port,
        };
        config.validate()?;
        Ok(config)
    }

    /// Creates a TelemetryConfig from environment variables.
    ///
    /// # Panics
    ///
    /// Panics if required environment variables (FLATBED_SERVICE_NAME, FLATBED_SERVICE_ADDRESS)
    /// are not set.
    pub fn from_env() -> Self {
        match Self::new(None, None, None) {
            Ok(instance) => instance,
            Err(err) => panic!("{:?}", err),
        }
    }

    /// Validates the telemetry configuration.
    pub fn validate(&self) -> Result<(), TelemetryError> {
        if self.service_name.trim().is_empty() {
            return Err(TelemetryError::ConfigValidationError(
                "Service name cannot be empty".to_string(),
            ));
        }

        if self.ip_address.parse::<std::net::IpAddr>().is_err() {
            return Err(TelemetryError::ConfigValidationError(
                "Invalid IP address format".to_string(),
            ));
        }

        if self.port == 0 {
            return Err(TelemetryError::ConfigValidationError(
                "Port number must be greater than 0".to_string(),
            ));
        }

        Ok(())
    }
}
