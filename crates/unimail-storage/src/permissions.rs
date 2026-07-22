use std::{fs, io, path::Path};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

pub(crate) fn ensure_private_directory(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)?;
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "private directory is not a regular directory",
        ));
    }
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

pub(crate) fn ensure_private_file_if_present(path: &Path) -> io::Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "private file is not a regular file",
        ));
    }
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(unix)]
pub(crate) fn configure_private_file_creation(options: &mut fs::OpenOptions) {
    options.mode(0o600);
}

#[cfg(not(unix))]
pub(crate) fn configure_private_file_creation(_: &mut fs::OpenOptions) {}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::{PermissionsExt, symlink};

    use tempfile::tempdir;

    use super::{ensure_private_directory, ensure_private_file_if_present};

    #[test]
    fn private_permissions_are_corrected_without_following_symlinks() {
        let directory = tempdir().expect("temporary directory");
        let private_directory = directory.path().join("profile");
        std::fs::create_dir(&private_directory).expect("profile directory");
        std::fs::set_permissions(&private_directory, std::fs::Permissions::from_mode(0o777))
            .expect("widen directory");
        ensure_private_directory(&private_directory).expect("correct directory");
        assert_eq!(
            std::fs::metadata(&private_directory)
                .expect("directory metadata")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );

        let private_file = private_directory.join("mail.db");
        std::fs::write(&private_file, b"encrypted").expect("private file");
        std::fs::set_permissions(&private_file, std::fs::Permissions::from_mode(0o666))
            .expect("widen file");
        ensure_private_file_if_present(&private_file).expect("correct file");
        assert_eq!(
            std::fs::metadata(&private_file)
                .expect("file metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );

        let target = private_directory.join("target");
        std::fs::write(&target, b"unrelated").expect("target");
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o644))
            .expect("target permissions");
        let link = private_directory.join("link");
        symlink(&target, &link).expect("symlink");
        assert!(ensure_private_file_if_present(&link).is_err());
        assert_eq!(
            std::fs::metadata(&target)
                .expect("target metadata")
                .permissions()
                .mode()
                & 0o777,
            0o644
        );
    }
}
