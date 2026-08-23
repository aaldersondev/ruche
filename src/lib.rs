//! Ruche : lancer plusieurs clients Minecraft cote a cote sans saturer
//! la machine.
//!
//! Le binaire n'est qu'une fenetre posee sur ces modules ; ils sont publics
//! pour que les tests d'integration (et un eventuel autre frontal) puissent
//! s'en servir.

pub mod app;
pub mod auth;
pub mod config;
pub mod mc;
pub mod queue;
pub mod sys;
