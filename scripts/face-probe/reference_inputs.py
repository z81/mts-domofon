"""Reference file discovery without importing inference libraries or decoding images."""

IMAGE_SUFFIXES = {".jpg", ".jpeg", ".png", ".webp"}


def reference_files(source):
    """One image, a flat album, or one subdirectory per reference identity."""
    if source.is_file():
        if source.suffix.lower() not in IMAGE_SUFFIXES:
            raise ValueError("Reference images must be JPEG, PNG or WebP")
        return [("reference", source, source.name)]
    if not source.is_dir():
        raise ValueError(f"Reference path does not exist: {source}")
    files = sorted(
        path for path in source.rglob("*")
        if path.is_file() and path.suffix.lower() in IMAGE_SUFFIXES
        and not any(part.startswith(".") for part in path.relative_to(source).parts)
    )
    if not files:
        raise ValueError("No JPEG, PNG or WebP reference images found")
    flat = [path.parent == source for path in files]
    if any(flat) and not all(flat):
        raise ValueError("Do not mix top-level photos and person subdirectories")
    return [
        (
            source.name if all(flat) else path.relative_to(source).parts[0],
            path,
            path.relative_to(source).as_posix(),
        )
        for path in files
    ]
