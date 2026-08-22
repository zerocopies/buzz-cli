pub fn print_banner() {
    let orange = "\x1b[38;5;208m";
    let white  = "\x1b[97m";
    let black  = "\x1b[90m";
    let reset  = "\x1b[0m";
    let bold   = "\x1b[1m";

    let body_widths: [usize; 9] = [6, 10, 14, 18, 20, 18, 14, 10, 6];
    let field_width = 20usize;

    // (black, white, white, black) columns per stripe, head-band -> mid-band -> tail-band
    let bands: [(usize, usize, usize, usize); 3] = [
        (3, 4, 5, 6),
        (8, 9, 10, 11),
        (13, 14, 15, 16),
    ];

    let tail_widths: [usize; 7] = [2, 4, 6, 8, 6, 4, 2];

    for (row, &width) in body_widths.iter().enumerate() {
        let pad = (field_width - width) / 2;
        let mut line = String::new();
        line.push_str(&" ".repeat(pad));

        for col in pad..(pad + width) {
            let mut ch_color = orange;
            for &(b, w1, w2, blk2) in bands.iter() {
                if col == b || col == blk2 {
                    ch_color = black;
                } else if col == w1 || col == w2 {
                    ch_color = white;
                }
            }
            if row == 3 && col == pad {
                ch_color = black; // eye
            }
            line.push_str(ch_color);
            line.push('█');
            line.push_str(reset);
        }

        if (1..=7).contains(&row) {
            let t_width = tail_widths[row - 1];
            line.push_str(orange);
            line.push_str(&"█".repeat(t_width));
            line.push_str(reset);
        }

        println!("{line}");
    }

    println!();
    println!("{bold}{orange}buzz{reset}{bold}{white}-{reset}{bold}{orange}cli{reset}");
}
