pub fn print_banner() {
    let orange = "\x1b[38;5;208m";
    let white  = "\x1b[97m";
    let black  = "\x1b[90m";
    let reset  = "\x1b[0m";
    let bold   = "\x1b[1m";

    let body_rows: [(usize, usize); 9] = [
        (6, 17),
        (4, 19),
        (2, 21),
        (1, 22),
        (0, 22),
        (1, 22),
        (2, 21),
        (4, 19),
        (6, 17),
    ];

    let tail_rows: [Option<(usize, usize)>; 9] = [
        None, None, None,
        Some((24, 26)),
        Some((24, 27)),
        Some((24, 26)),
        None, None, None,
    ];

    let stripes: [(usize, usize, usize, usize); 3] = [
        (4, 5, 6, 7),
        (11, 12, 13, 14),
        (17, 18, 19, 20),
    ];

    let eye = (2usize, 3usize);

    for row in 0..9 {
        let (start, end) = body_rows[row];
        let tail = tail_rows[row];
        let max_col = end.max(tail.map(|(_, e)| e).unwrap_or(0));
        let mut line = String::new();

        for col in 0..=max_col {
            let in_body = col >= start && col <= end;
            let in_tail = tail.map_or(false, |(s, e)| col >= s && col <= e);

            if !in_body && !in_tail {
                line.push(' ');
                continue;
            }

            let mut color = orange;
            if in_body {
                if (row, col) == eye {
                    color = black;
                } else {
                    for &(b1, w1, w2, b2) in stripes.iter() {
                        if col == b1 || col == b2 {
                            color = black;
                        } else if col == w1 || col == w2 {
                            color = white;
                        }
                    }
                }
            }
            line.push_str(color);
            line.push('█');
            line.push_str(reset);
        }
        println!("{line}");
    }

    println!();
    println!("{bold}{orange}buzz{reset}{bold}{white}-{reset}{bold}{orange}cli{reset}");
}
