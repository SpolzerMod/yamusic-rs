//! Извлечение FLAC из MP4 и запись метаданных без перекодирования.

pub fn prepare(data: &[u8], tags: &crate::tags::Tags<'_>) -> Result<Vec<u8>, String> {
    let (metadata, audio) = if data.starts_with(b"fLaC") {
        let (blocks, end) = metadata_blocks(&data[4..])?;
        (blocks, data[4 + end..].to_vec())
    } else {
        demux(data)?
    };
    if audio.is_empty() || audio.len() < 2 || audio[0] != 255 || audio[1] & 254 != 248 {
        return Err("Не найдено аудиоданных FLAC".into());
    }
    let mut blocks: Vec<(u8, Vec<u8>)> = metadata.into_iter().filter(|(kind, _)| *kind != 4 && *kind != 6).collect();
    blocks.push((4, comments(tags)?));
    if let Some(cover) = tags.cover {
        blocks.push((6, picture(cover)?));
    }
    let mut output = b"fLaC".to_vec();
    for (index, (kind, body)) in blocks.iter().enumerate() {
        if body.len() > 0xff_ffff {
            return Err("Слишком большой блок метаданных FLAC".into());
        }
        output.push(*kind | if index + 1 == blocks.len() { 128 } else { 0 });
        output.extend_from_slice(&(body.len() as u32).to_be_bytes()[1..]);
        output.extend_from_slice(body);
    }
    output.extend(audio);
    Ok(output)
}

type Metadata = Vec<(u8, Vec<u8>)>;

fn metadata_blocks(data: &[u8]) -> Result<(Metadata, usize), String> {
    let mut blocks = Vec::new();
    let mut pos = 0;
    loop {
        let header = data.get(pos..pos + 4).ok_or("Обрезанный заголовок FLAC")?;
        let kind = header[0] & 127;
        let size = u32::from_be_bytes([0, header[1], header[2], header[3]]) as usize;
        let end = pos.checked_add(4 + size).ok_or("Некорректный размер FLAC")?;
        let body = data.get(pos + 4..end).ok_or("Обрезанные метаданные FLAC")?;
        if (blocks.is_empty() && (kind != 0 || size != 34)) || (!blocks.is_empty() && kind == 0) || kind == 127 {
            return Err("Некорректный STREAMINFO или тип блока FLAC".into());
        }
        blocks.push((kind, body.to_vec()));
        pos = end;
        if header[0] & 128 != 0 {
            return Ok((blocks, pos));
        }
    }
}

#[derive(Clone, Copy)]
struct Atom {
    kind: [u8; 4],
    payload: usize,
    end: usize,
}

fn be32(data: &[u8], pos: usize) -> Result<u32, String> {
    Ok(u32::from_be_bytes(data.get(pos..pos.checked_add(4).ok_or("Переполнение MP4")?).ok_or("Обрезанное поле MP4")?.try_into().unwrap()))
}

fn atoms(data: &[u8], start: usize, end: usize) -> Result<Vec<Atom>, String> {
    if end > data.len() || start > end {
        return Err("Некорректные границы MP4".into());
    }
    let mut pos = start;
    let mut out = Vec::new();
    while pos < end {
        if end - pos < 8 {
            return Err("Обрезанный заголовок контейнера MP4".into());
        }
        let small = be32(data, pos)?;
        let (size, header) = if small == 1 {
            if end - pos < 16 {
                return Err("Обрезанный расширенный размер MP4".into());
            }
            let size = u64::from_be_bytes(data[pos + 8..pos + 16].try_into().unwrap());
            (usize::try_from(size).map_err(|_| "Слишком большой контейнер MP4")?, 16)
        } else if small == 0 {
            (end - pos, 8)
        } else {
            (small as usize, 8)
        };
        let next = pos.checked_add(size).ok_or("Переполнение размера MP4")?;
        if size < header || next > end {
            return Err("Некорректный размер контейнера MP4".into());
        }
        out.push(Atom { kind: data[pos + 4..pos + 8].try_into().unwrap(), payload: pos + header, end: next });
        pos = next;
    }
    Ok(out)
}

fn one(list: &[Atom], kind: &[u8; 4]) -> Result<Atom, String> {
    let mut matching = list.iter().filter(|a| a.kind == *kind);
    let found = *matching.next().ok_or_else(|| format!("Отсутствует {} в MP4", String::from_utf8_lossy(kind)))?;
    if matching.next().is_some() {
        return Err("Повторяющийся контейнер MP4".into());
    }
    Ok(found)
}

fn full_body(data: &[u8], atom: Atom) -> Result<&[u8], String> {
    let body = &data[atom.payload..atom.end];
    if body.len() < 4 || body[..4] != [0; 4] {
        return Err("Неподдерживаемая версия контейнера MP4".into());
    }
    Ok(&body[4..])
}

fn sized_table(body: &[u8], offset: usize, count: usize, width: usize) -> Result<(), String> {
    let size = count.checked_mul(width).and_then(|v| v.checked_add(offset)).ok_or("Переполнение таблицы MP4")?;
    if size != body.len() {
        return Err("Некорректная длина таблицы MP4".into());
    }
    Ok(())
}

fn demux(data: &[u8]) -> Result<(Metadata, Vec<u8>), String> {
    let top = atoms(data, 0, data.len())?;
    if top.iter().any(|a| a.kind == *b"moof") {
        return Err("Фрагментированный MP4 не поддерживается".into());
    }
    let moov = one(&top, b"moov")?;
    let movies = atoms(data, moov.payload, moov.end)?;
    if movies.iter().any(|a| a.kind == *b"mvex") {
        return Err("Фрагментированный MP4 не поддерживается".into());
    }
    let media: Vec<_> = top.iter().filter(|a| a.kind == *b"mdat").copied().collect();
    if media.is_empty() {
        return Err("Отсутствует mdat в MP4".into());
    }
    for track in movies.iter().filter(|a| a.kind == *b"trak") {
        let children = atoms(data, track.payload, track.end)?;
        let mdia = one(&children, b"mdia")?;
        let children = atoms(data, mdia.payload, mdia.end)?;
        if let Some(hdlr) = children.iter().find(|a| a.kind == *b"hdlr") {
            let handler = full_body(data, *hdlr)?;
            if handler.get(4..8) != Some(b"soun".as_slice()) {
                continue;
            }
        }
        let minf = one(&children, b"minf")?;
        let children = atoms(data, minf.payload, minf.end)?;
        let stbl = one(&children, b"stbl")?;
        let tables = atoms(data, stbl.payload, stbl.end)?;
        let Some(stsd) = tables.iter().find(|a| a.kind == *b"stsd") else {
            continue;
        };
        let body = full_body(data, *stsd)?;
        let entry_count = be32(body, 0)? as usize;
        let entries = atoms(data, stsd.payload + 8, stsd.end)?;
        if entries.len() != entry_count {
            return Err("Некорректное число описаний MP4".into());
        }
        let Some((index, entry)) = entries.iter().enumerate().find(|(_, a)| a.kind == *b"fLaC") else {
            continue;
        };
        if tables.iter().any(|a| matches!(&a.kind, b"senc" | b"saiz" | b"saio")) {
            return Err("Зашифрованный MP4 не поддерживается".into());
        }
        let header = data.get(entry.payload..entry.payload + 28).filter(|_| entry.end >= entry.payload + 28).ok_or("Обрезанное описание FLAC")?;
        if header[8..10] != [0, 0] || header[6..8] != [0, 1] {
            return Err("Неподдерживаемое описание FLAC в MP4".into());
        }
        check_data_reference(data, &children)?;
        let configs = atoms(data, entry.payload + 28, entry.end)?;
        if configs.iter().any(|a| a.kind == *b"sinf") {
            return Err("Зашифрованный MP4 не поддерживается".into());
        }
        let config = one(&configs, b"dfLa")?;
        let body = full_body(data, config)?;
        let (metadata, used) = metadata_blocks(body)?;
        if used != body.len() {
            return Err("Лишние данные в конфигурации FLAC".into());
        }
        let audio = extract_samples(data, &tables, &media, index + 1)?;
        return Ok((metadata, audio));
    }
    Err("В MP4 нет незашифрованного звукового трека FLAC".into())
}

fn check_data_reference(data: &[u8], minf: &[Atom]) -> Result<(), String> {
    // Some files omit dinf and put the samples directly in mdat. An explicit
    // external reference must never be interpreted as an offset in this file.
    if !minf.iter().any(|a| a.kind == *b"dinf") {
        return Ok(());
    }
    let dinf = one(minf, b"dinf")?;
    let children = atoms(data, dinf.payload, dinf.end)?;
    let dref = one(&children, b"dref")?;
    let body = full_body(data, dref)?;
    let count = be32(body, 0)? as usize;
    let entries = atoms(data, dref.payload + 8, dref.end)?;
    if count != entries.len() {
        return Err("Некорректная таблица ссылок MP4".into());
    }
    let reference = entries.first().ok_or("Отсутствует ссылка на аудиоданные MP4")?;
    if reference.kind != *b"url " || data[reference.payload..reference.end] != [0, 0, 0, 1] {
        return Err("Внешние аудиоданные MP4 не поддерживаются".into());
    }
    Ok(())
}

fn extract_samples(data: &[u8], tables: &[Atom], media: &[Atom], description: usize) -> Result<Vec<u8>, String> {
    let sizes = full_body(data, one(tables, b"stsz")?)?;
    let fixed = be32(sizes, 0)? as usize;
    let count = be32(sizes, 4)? as usize;
    sized_table(sizes, 8, if fixed == 0 { count } else { 0 }, 4)?;
    if count == 0 || count > data.len() {
        return Err("Некорректное число сэмплов FLAC".into());
    }
    let sample_sizes: Vec<usize> =
        (0..count).map(|i| if fixed == 0 { be32(sizes, 8 + i * 4).map(|v| v as usize) } else { Ok(fixed) }).collect::<Result<_, _>>()?;
    let total =
        sample_sizes.iter().try_fold(0usize, |sum, size| if *size == 0 { None } else { sum.checked_add(*size) }).ok_or("Некорректный размер сэмплов FLAC")?;
    if total > data.len() {
        return Err("Размер сэмплов превышает размер MP4".into());
    }
    let narrow = tables.iter().filter(|a| a.kind == *b"stco").count();
    let wide = tables.iter().filter(|a| a.kind == *b"co64").count();
    if narrow + wide != 1 {
        return Err("Неоднозначная или отсутствующая таблица смещений MP4".into());
    }
    let offsets = full_body(data, one(tables, if wide == 1 { b"co64" } else { b"stco" })?)?;
    let chunks = be32(offsets, 0)? as usize;
    let width = if wide == 1 { 8 } else { 4 };
    sized_table(offsets, 4, chunks, width)?;
    if chunks == 0 || chunks > count {
        return Err("Некорректное число чанков MP4".into());
    }
    let mapping = full_body(data, one(tables, b"stsc")?)?;
    let run_count = be32(mapping, 0)? as usize;
    sized_table(mapping, 4, run_count, 12)?;
    let mut runs = Vec::new();
    for i in 0..run_count {
        let first = be32(mapping, 4 + i * 12)? as usize;
        let samples = be32(mapping, 8 + i * 12)? as usize;
        let sample_description = be32(mapping, 12 + i * 12)? as usize;
        if first == 0
            || first > chunks
            || samples == 0
            || sample_description != description
            || (i == 0 && first != 1)
            || runs.last().is_some_and(|(previous, _)| *previous >= first)
        {
            return Err("Некорректная таблица stsc или смена кодека в MP4".into());
        }
        runs.push((first, samples));
    }
    if runs.is_empty() {
        return Err("Пустая таблица stsc".into());
    }
    let mut result = Vec::with_capacity(total);
    let mut sample = 0usize;
    let mut run = 0;
    let mut ranges = Vec::with_capacity(chunks);
    for chunk in 0..chunks {
        while run + 1 < runs.len() && runs[run + 1].0 <= chunk + 1 {
            run += 1;
        }
        let next_sample = sample.checked_add(runs[run].1).filter(|v| *v <= count).ok_or("stsc ссылается на отсутствующие сэмплы")?;
        let offset = if wide == 1 {
            let pos = 4 + chunk * 8;
            usize::try_from(u64::from_be_bytes(offsets[pos..pos + 8].try_into().unwrap())).map_err(|_| "Переполнение смещения MP4")?
        } else {
            be32(offsets, 4 + chunk * 4)? as usize
        };
        let len: usize = sample_sizes[sample..next_sample].iter().sum();
        let end = offset.checked_add(len).ok_or("Переполнение границ сэмпла MP4")?;
        if !media.iter().any(|a| offset >= a.payload && end <= a.end) {
            return Err("Сэмпл FLAC выходит за границы mdat".into());
        }
        ranges.push((offset, end));
        result.extend_from_slice(&data[offset..end]);
        sample = next_sample;
    }
    if sample != count {
        return Err("Не все сэмплы FLAC описаны таблицей stsc".into());
    }
    ranges.sort_unstable();
    if ranges.windows(2).any(|pair| pair[0].1 > pair[1].0) {
        return Err("Пересекающиеся чанки FLAC в MP4".into());
    }
    Ok(result)
}

fn append_length(out: &mut Vec<u8>, value: &[u8], little: bool) -> Result<(), String> {
    let len = u32::try_from(value.len()).map_err(|_| "Слишком длинное поле метаданных FLAC")?;
    out.extend_from_slice(&if little { len.to_le_bytes() } else { len.to_be_bytes() });
    out.extend_from_slice(value);
    Ok(())
}

fn comments(tags: &crate::tags::Tags<'_>) -> Result<Vec<u8>, String> {
    let mut entries = Vec::new();
    for (name, text) in [("TITLE", tags.title), ("ARTIST", tags.artist), ("ALBUM", tags.album)] {
        if !text.is_empty() {
            entries.push(format!("{name}={text}"));
        }
    }
    if let Some(year) = tags.year {
        entries.push(format!("DATE={year}"));
    }
    if let Some(track) = tags.track {
        entries.push(format!("TRACKNUMBER={track}"));
    }
    let mut out = Vec::new();
    append_length(&mut out, b"yamusic-rs", true)?;
    out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    for entry in entries {
        append_length(&mut out, entry.as_bytes(), true)?;
    }
    Ok(out)
}

fn picture(cover: &[u8]) -> Result<Vec<u8>, String> {
    let info = image_info(cover)?;
    let mut out = 3u32.to_be_bytes().to_vec();
    append_length(&mut out, info.mime.as_bytes(), false)?;
    append_length(&mut out, b"", false)?;
    for value in [info.width, info.height, info.depth, info.colors] {
        out.extend_from_slice(&value.to_be_bytes());
    }
    append_length(&mut out, cover, false)?;
    Ok(out)
}

struct ImageInfo {
    mime: &'static str,
    width: u32,
    height: u32,
    depth: u32,
    colors: u32,
}

fn image_info(data: &[u8]) -> Result<ImageInfo, String> {
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        if data.len() < 33 || be32(data, 8)? != 13 || &data[12..16] != b"IHDR" {
            return Err("Обрезанный заголовок обложки PNG".into());
        }
        let width = be32(data, 16)?;
        let height = be32(data, 20)?;
        let bits = data[24] as u32;
        let channels = match data[25] {
            0 | 3 => 1,
            2 => 3,
            4 => 2,
            6 => 4,
            _ => return Err("Некорректный тип цвета PNG".into()),
        };
        let valid_bits = match data[25] {
            0 => matches!(bits, 1 | 2 | 4 | 8 | 16),
            3 => matches!(bits, 1 | 2 | 4 | 8),
            _ => matches!(bits, 8 | 16),
        };
        if width == 0 || height == 0 || !valid_bits {
            return Err("Некорректные параметры обложки PNG".into());
        }
        let mut colors = 0;
        if data[25] == 3 {
            let mut pos = 33;
            while pos < data.len() {
                let size = be32(data, pos)? as usize;
                let end = pos.checked_add(size).and_then(|v| v.checked_add(12)).ok_or("Переполнение размера PNG")?;
                let chunk = data.get(pos..end).ok_or("Обрезанная палитра PNG")?;
                if &chunk[4..8] == b"PLTE" {
                    if size == 0 || !size.is_multiple_of(3) || size / 3 > 1usize << bits {
                        return Err("Некорректная палитра PNG".into());
                    }
                    colors = (size / 3) as u32;
                    break;
                }
                if &chunk[4..8] == b"IDAT" {
                    break;
                }
                pos = end;
            }
            if colors == 0 {
                return Err("Отсутствует палитра PNG".into());
            }
        }
        return Ok(ImageInfo { mime: "image/png", width, height, depth: bits * channels, colors });
    }
    if data.starts_with(&[255, 216]) {
        let mut pos = 2;
        while pos < data.len() {
            if data[pos] != 255 {
                return Err("Некорректный маркер JPEG".into());
            }
            while data.get(pos) == Some(&255) {
                pos += 1;
            }
            let marker = *data.get(pos).ok_or("Обрезанный маркер JPEG")?;
            pos += 1;
            if marker == 217 || marker == 218 {
                break;
            }
            if marker == 1 || (208..=215).contains(&marker) {
                continue;
            }
            let length = data.get(pos..pos + 2).ok_or("Обрезанный сегмент JPEG")?;
            let len = u16::from_be_bytes(length.try_into().unwrap()) as usize;
            if len < 2 {
                return Err("Некорректный размер сегмента JPEG".into());
            }
            let segment = data.get(pos..pos + len).ok_or("Обрезанная обложка JPEG")?;
            if (192..=207).contains(&marker) && ![196, 200, 204].contains(&marker) {
                if len < 8 {
                    return Err("Обрезанный заголовок кадра JPEG".into());
                }
                let height = u16::from_be_bytes([segment[3], segment[4]]) as u32;
                let width = u16::from_be_bytes([segment[5], segment[6]]) as u32;
                let channels = segment[7] as usize;
                if width == 0 || height == 0 || segment[2] == 0 || channels == 0 || len != 8 + 3 * channels {
                    return Err("Некорректные параметры обложки JPEG".into());
                }
                return Ok(ImageInfo { mime: "image/jpeg", width, height, depth: segment[2] as u32 * channels as u32, colors: 0 });
            }
            pos += len;
        }
        return Err("Не найдены размеры обложки JPEG".into());
    }
    Err("Обложка должна быть JPEG или PNG".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags() -> crate::tags::Tags<'static> {
        crate::tags::Tags { title: "Песня", artist: "Автор", album: "Альбом", year: Some(2026), track: Some(7), cover: None }
    }

    fn block(kind: u8, last: bool, payload: &[u8]) -> Vec<u8> {
        let mut out = vec![kind | if last { 128 } else { 0 }];
        out.extend_from_slice(&(payload.len() as u32).to_be_bytes()[1..]);
        out.extend_from_slice(payload);
        out
    }

    fn metadata() -> Vec<u8> {
        let mut out = block(0, false, &[0; 34]);
        out.extend(block(2, true, b"test-preserved"));
        out
    }

    fn inspect(data: &[u8]) -> (Vec<(u8, &[u8])>, &[u8]) {
        assert_eq!(&data[..4], b"fLaC");
        let mut offset = 4;
        let mut blocks = Vec::new();
        loop {
            let header = data[offset];
            let len = u32::from_be_bytes([0, data[offset + 1], data[offset + 2], data[offset + 3]]) as usize;
            blocks.push((header & 127, &data[offset + 4..offset + 4 + len]));
            offset += 4 + len;
            if header & 128 != 0 {
                break;
            }
        }
        (blocks, &data[offset..])
    }

    fn atom(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut out = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
        out.extend_from_slice(kind);
        out.extend_from_slice(payload);
        out
    }

    fn full(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        atom(kind, &[&[0; 4][..], payload].concat())
    }

    // Two selected chunks separated by bytes belonging to another track, and
    // another track's sample tables placed before those of the FLAC track.
    fn mp4(fixed: bool, wide: bool) -> (Vec<u8>, Vec<u8>) {
        let sizes: Vec<u32> = if fixed { vec![2, 2, 2] } else { vec![3, 2, 4] };
        let audio = if fixed { vec![255, 248, 11, 12, 13, 14] } else { vec![255, 248, 11, 12, 13, 14, 15, 16, 17] };
        let mut header = vec![0; 28];
        header[7] = 1; // local data_reference_index
        let flac_entry = atom(b"fLaC", &[header, full(b"dfLa", &metadata())].concat());
        let stsd = full(b"stsd", &[2u32.to_be_bytes().to_vec(), atom(b"mp4a", &[0; 28]), flac_entry].concat());
        let mut stsz = [if fixed { 2u32 } else { 0 }.to_be_bytes(), 3u32.to_be_bytes()].concat();
        if !fixed {
            for size in &sizes {
                stsz.extend(size.to_be_bytes());
            }
        }
        let stsc = full(b"stsc", &[2u32, 1, 2, 2, 2, 1, 2].iter().flat_map(|x| x.to_be_bytes()).collect::<Vec<_>>());
        let offsets = full(if wide { b"co64" } else { b"stco" }, &[2u32.to_be_bytes().to_vec(), vec![0; if wide { 16 } else { 8 }]].concat());
        let stbl = atom(b"stbl", &[stsd, full(b"stsz", &stsz), stsc, offsets].concat());
        let hdlr = full(b"hdlr", &[&[0; 4][..], b"soun"].concat());
        let dref = full(b"dref", &[1u32.to_be_bytes().to_vec(), atom(b"url ", &[0, 0, 0, 1])].concat());
        let selected = atom(b"trak", &atom(b"mdia", &[hdlr, atom(b"minf", &[atom(b"dinf", &dref), stbl].concat())].concat()));
        let unrelated = atom(b"trak", &atom(b"mdia", &atom(b"minf", &atom(b"stbl", &full(b"stsz", &[0; 8])))));
        let mut data = atom(b"moov", &[unrelated, selected].concat());
        let split = (sizes[0] + sizes[1]) as usize;
        let mdat_start = data.len() + 8;
        let first = mdat_start + 3;
        let second = first + split + 5;
        let payload = [b"xxx".as_slice(), &audio[..split], b"other", &audio[split..], b"tail"].concat();
        let pos = data.windows(4).position(|p| p == if wide { b"co64" } else { b"stco" }).unwrap() + 12;
        if wide {
            data[pos..pos + 8].copy_from_slice(&(first as u64).to_be_bytes());
            data[pos + 8..pos + 16].copy_from_slice(&(second as u64).to_be_bytes());
        } else {
            data[pos..pos + 4].copy_from_slice(&(first as u32).to_be_bytes());
            data[pos + 4..pos + 8].copy_from_slice(&(second as u32).to_be_bytes());
        }
        data.extend(atom(b"mdat", &payload));
        (data, audio)
    }

    #[test]
    fn raw_replaces_comments_and_picture_preserving_other_blocks_and_audio() {
        let audio = [255, 248, 4, 3, 2, 1];
        let mut data = b"fLaC".to_vec();
        data.extend(block(0, false, &[1; 34]));
        data.extend(block(4, false, b"old-comments"));
        data.extend(block(2, false, b"application"));
        data.extend(block(6, true, b"old-cover"));
        data.extend(audio);
        let output = prepare(&data, &tags()).unwrap();
        let (blocks, frames) = inspect(&output);
        assert_eq!(frames, audio);
        assert_eq!(blocks.iter().map(|b| b.0).collect::<Vec<_>>(), [0, 2, 4]);
        assert_eq!(blocks[0].1, [1; 34]);
        assert_eq!(blocks[1].1, b"application");
        for comment in ["TITLE=Песня", "ARTIST=Автор", "ALBUM=Альбом", "DATE=2026", "TRACKNUMBER=7"] {
            assert!(blocks[2].1.windows(comment.len()).any(|w| w == comment.as_bytes()));
        }
    }

    #[test]
    fn mp4_variable_samples_follow_selected_track_tables_and_chunk_runs() {
        let (data, audio) = mp4(false, false);
        let output = prepare(&data, &tags()).unwrap();
        let (blocks, frames) = inspect(&output);
        assert_eq!(frames, audio);
        assert_eq!(blocks[1].1, b"test-preserved");
    }

    #[test]
    fn mp4_fixed_samples_and_co64_offsets() {
        let (data, audio) = mp4(true, true);
        let output = prepare(&data, &tags()).unwrap();
        assert_eq!(inspect(&output).1, audio);
    }

    #[test]
    fn rejects_sample_outside_mdat_and_truncated_sample_tables() {
        let (mut data, _) = mp4(false, false);
        let pos = data.windows(4).position(|p| p == b"stco").unwrap() + 12;
        data[pos..pos + 4].copy_from_slice(&12u32.to_be_bytes());
        assert!(prepare(&data, &tags()).is_err());
        let (mut data, _) = mp4(false, false);
        let pos = data.windows(4).rposition(|p| p == b"stsz").unwrap() + 12;
        data[pos..pos + 4].copy_from_slice(&100u32.to_be_bytes());
        assert!(prepare(&data, &tags()).is_err());
    }

    #[test]
    fn rejects_oversized_offsets_fragments_and_encryption() {
        let (mut data, _) = mp4(true, true);
        let pos = data.windows(4).position(|p| p == b"co64").unwrap() + 12;
        data[pos..pos + 8].copy_from_slice(&u64::MAX.to_be_bytes());
        assert!(prepare(&data, &tags()).is_err());
        let (mut data, _) = mp4(false, false);
        data.extend(atom(b"moof", &[]));
        assert!(prepare(&data, &tags()).is_err());
        let (mut data, _) = mp4(false, false);
        let pos = data.windows(4).position(|p| p == b"fLaC").unwrap();
        data[pos..pos + 4].copy_from_slice(b"enca");
        assert!(prepare(&data, &tags()).is_err());
    }

    #[test]
    fn rejects_wrong_chunk_mapping_overlaps_and_end_bounds() {
        for (field, value) in [(4, 0u32), (8, 3), (12, 1), (20, 2)] {
            let (mut data, _) = mp4(false, false);
            let body = data.windows(4).position(|p| p == b"stsc").unwrap() + 8;
            data[body + field..body + field + 4].copy_from_slice(&value.to_be_bytes());
            assert!(prepare(&data, &tags()).is_err(), "stsc field {field}");
        }
        let (mut data, _) = mp4(false, false);
        let offset = data.windows(4).position(|p| p == b"stco").unwrap() + 12;
        let first = data[offset..offset + 4].to_vec();
        data[offset + 4..offset + 8].copy_from_slice(&first);
        assert!(prepare(&data, &tags()).is_err());
        let (mut data, _) = mp4(false, false);
        let offset = data.windows(4).position(|p| p == b"stco").unwrap() + 12;
        let last_byte = (data.len() - 1) as u32;
        data[offset + 4..offset + 8].copy_from_slice(&last_byte.to_be_bytes());
        assert!(prepare(&data, &tags()).is_err());
    }

    #[test]
    fn rejects_every_truncated_prefix_without_panicking() {
        let (data, _) = mp4(false, true);
        for len in 0..data.len() {
            assert!(prepare(&data[..len], &tags()).is_err(), "prefix {len}");
        }
    }

    #[test]
    fn rejects_encryption_boxes_and_missing_codec_configuration() {
        for replacement in [b"sinf", b"free"] {
            let (mut data, _) = mp4(false, false);
            let pos = data.windows(4).position(|p| p == b"dfLa").unwrap();
            data[pos..pos + 4].copy_from_slice(replacement);
            assert!(prepare(&data, &tags()).is_err());
        }
        let (mut data, _) = mp4(false, false);
        let pos = data.windows(4).rposition(|p| p == b"stsz").unwrap();
        data[pos..pos + 4].copy_from_slice(b"senc");
        assert!(prepare(&data, &tags()).is_err());
    }

    #[test]
    fn rejects_external_media_reference() {
        let (mut data, _) = mp4(false, false);
        let pos = data.windows(4).position(|p| p == b"url ").unwrap() + 7;
        data[pos] = 0;
        assert!(prepare(&data, &tags()).is_err());
    }

    #[test]
    fn rejects_truncated_flac_and_wrong_formats() {
        for data in [b"fLaC\x80\x00\x00\x22short".as_slice(), b"ID3mp3", b"fLaC\x84\x00\x00\x00"] {
            assert!(prepare(data, &tags()).is_err());
        }
    }

    #[test]
    fn png_picture_has_dimensions_and_original_image() {
        let png =
            [b"\x89PNG\r\n\x1a\n".as_slice(), &13u32.to_be_bytes(), b"IHDR", &400u32.to_be_bytes(), &600u32.to_be_bytes(), &[8, 2, 0, 0, 0], &[0; 4]].concat();
        let mut t = tags();
        t.cover = Some(&png);
        let data = [b"fLaC".as_slice(), &metadata(), &[255, 248, 1, 2]].concat();
        let output = prepare(&data, &t).unwrap();
        let (blocks, _) = inspect(&output);
        let picture = blocks.iter().find(|b| b.0 == 6).unwrap().1;
        assert_eq!(u32::from_be_bytes(picture[..4].try_into().unwrap()), 3);
        assert_eq!(&picture[8..17], b"image/png");
        assert_eq!(u32::from_be_bytes(picture[21..25].try_into().unwrap()), 400);
        assert_eq!(u32::from_be_bytes(picture[25..29].try_into().unwrap()), 600);
        assert_eq!(u32::from_be_bytes(picture[29..33].try_into().unwrap()), 24);
        assert!(picture.ends_with(&png));
    }

    #[test]
    fn jpeg_picture_has_dimensions_and_depth() {
        let jpeg = [255, 216, 255, 224, 0, 4, 1, 2, 255, 192, 0, 17, 8, 2, 88, 1, 144, 3, 1, 17, 0, 2, 17, 0, 3, 17, 0, 255, 217];
        let mut t = tags();
        t.cover = Some(&jpeg);
        let data = [b"fLaC".as_slice(), &metadata(), &[255, 248, 1, 2]].concat();
        let output = prepare(&data, &t).unwrap();
        let (blocks, _) = inspect(&output);
        let picture = blocks.iter().find(|b| b.0 == 6).unwrap().1;
        assert_eq!(&picture[8..18], b"image/jpeg");
        assert_eq!(u32::from_be_bytes(picture[22..26].try_into().unwrap()), 400);
        assert_eq!(u32::from_be_bytes(picture[26..30].try_into().unwrap()), 600);
        assert_eq!(u32::from_be_bytes(picture[30..34].try_into().unwrap()), 24);
        assert!(picture.ends_with(&jpeg));
        t.cover = Some(&jpeg[..12]);
        assert!(prepare(&data, &t).is_err());
    }

    #[test]
    fn indexed_png_picture_records_palette_size() {
        let png = [
            b"\x89PNG\r\n\x1a\n".as_slice(),
            &13u32.to_be_bytes(),
            b"IHDR",
            &1u32.to_be_bytes(),
            &1u32.to_be_bytes(),
            &[8, 3, 0, 0, 0],
            &[0; 4],
            &6u32.to_be_bytes(),
            b"PLTE",
            &[255, 0, 0, 0, 255, 0],
            &[0; 4],
        ]
        .concat();
        let mut t = tags();
        t.cover = Some(&png);
        let data = [b"fLaC".as_slice(), &metadata(), &[255, 248, 1, 2]].concat();
        let output = prepare(&data, &t).unwrap();
        let (blocks, _) = inspect(&output);
        let picture = blocks.iter().find(|b| b.0 == 6).unwrap().1;
        assert_eq!(u32::from_be_bytes(picture[33..37].try_into().unwrap()), 2);
    }
}
