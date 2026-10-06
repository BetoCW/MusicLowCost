//! Conexiones del Jam por internet con iroh (ver mod.rs).

use super::{ALPN, room_key};
use iroh::endpoint::presets;
use iroh::{Endpoint, EndpointId};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};

pub type Rd = Box<dyn AsyncRead + Unpin>;
pub type Wr = Box<dyn AsyncWrite + Unpin>;
/// Lo que mantiene viva una conexion (endpoint, conexion QUIC...); se suelta al terminar.
pub type Keep = Box<dyn std::any::Any>;

/// Cuanto esperar a encontrar al anfitrion (busqueda en el directorio + hole punching).
const CONNECT_TIMEOUT: Duration = Duration::from_secs(25);
/// Para saber si ya hay alguien con esa sala abierta.
const PROBE_TIMEOUT: Duration = Duration::from_secs(6);

fn room_id(room: &str, password: &str) -> EndpointId {
    room_key(room, password).public()
}

/// Endpoint del anfitrion: su id sale del nombre y la contraseña de la sala.
pub async fn host_endpoint(room: &str, password: &str) -> Result<Endpoint, String> {
    // Si la sala ya existe (otra persona la abrio con el mismo nombre y contraseña),
    // dos anfitriones con la misma llave se pelearian por las conexiones.
    if let Ok(probe) = Endpoint::bind(presets::N0).await {
        let taken = tokio::time::timeout(PROBE_TIMEOUT, probe.connect(room_id(room, password), ALPN))
            .await
            .is_ok_and(|r| r.is_ok());
        probe.close().await;
        if taken {
            return Err("Ya hay un Jam abierto con ese nombre y contraseña: únete a él o usa otro nombre.".into());
        }
    }
    let ep = Endpoint::builder(presets::N0)
        .secret_key(room_key(room, password))
        .alpns(vec![ALPN.to_vec()])
        .bind()
        .await
        .map_err(|e| format!("No se pudo preparar la conexión: {e}"))?;
    // Publicarse en el directorio y en un relay antes de decir que esta listo.
    if tokio::time::timeout(Duration::from_secs(15), ep.online()).await.is_err() {
        log::warn!("jam: el endpoint tardó en quedar en línea");
    }
    Ok(ep)
}

/// Acepta la siguiente conexion y su stream bidireccional.
pub async fn accept(ep: &Endpoint) -> Option<Result<(Rd, Wr, Keep), String>> {
    let incoming = ep.accept().await?;
    Some(
        async move {
            let conn = incoming.await.map_err(|e| e.to_string())?;
            let (send, recv) = tokio::time::timeout(Duration::from_secs(10), conn.accept_bi())
                .await
                .map_err(|_| "el invitado no abrió el canal".to_string())?
                .map_err(|e| e.to_string())?;
            Ok((Box::new(recv) as Rd, Box::new(send) as Wr, Box::new(conn) as Keep))
        }
        .await,
    )
}

/// Se conecta a la sala; el invitado tiene una llave nueva cada vez.
pub async fn connect(room: &str, password: &str) -> Result<(Rd, Wr, Keep), String> {
    let ep = Endpoint::bind(presets::N0).await.map_err(|e| format!("No se pudo preparar la conexión: {e}"))?;
    let conn = match tokio::time::timeout(CONNECT_TIMEOUT, ep.connect(room_id(room, password), ALPN)).await {
        Ok(Ok(c)) => c,
        Ok(Err(e)) => {
            log::info!("jam: conectar: {e}");
            return Err(not_found());
        }
        Err(_) => return Err(not_found()),
    };
    let (send, recv) = conn.open_bi().await.map_err(|e| format!("No se pudo abrir el canal: {e}"))?;
    Ok((Box::new(recv), Box::new(send), Box::new((ep, conn))))
}

fn not_found() -> String {
    "No se encontró ese Jam. Revisa el nombre y la contraseña, y que el anfitrión lo tenga abierto.".into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn la_sala_no_distingue_mayusculas_pero_la_contrasena_si() {
        assert_eq!(room_id("Fiesta  de Ana", "x1"), room_id(" fiesta de ana ", "x1"));
        assert_ne!(room_id("fiesta", "x1"), room_id("fiesta", "X1"));
        assert_ne!(room_id("fiesta", "x1"), room_id("fiesta2", "x1"));
    }
}
