# httplatencies

I needed to measure latencies for an endpoint and I could have used [wrk](https://github.com/wg/wrk), but you
know NIH. And also I wanted to do a little [rust](https://rust-lang.org). So I created a little one-shot tool
and later polished it a bit up for this.

Have fun.

    httplatencies 1.0.0
    bombard endpoints with many get requests
    
    You request a number of tasks, each task will start doing get requests to one of the given urls.
    
    The urls are distributed on the tasks in round robin fashion. So if you have fewer tasks then urls, some urls will not
    be used.
    
    Having more than one url is mainly used to bombard the same server on different IPs, so we don't run out of ports.
    
    Also you can specify multiple local IPs for the http clients to use, each task will use one http client and will get it
    assigned in a round robin fashion on creation. Again this is meant to be used, if you run out of ports with one IP.
    
    USAGE:
        httplatencies [OPTIONS] --urls <urls>...
    
    FLAGS:
            --help       
                Prints help information
    
        -V, --version    
                Prints version information
    
    
    OPTIONS:
        -h, --headers-from-file <headers-from-file>...
                You can specify headers here to be added to the get requests. For each header you specify a file, which is
                expected to contain header values separated by newlines.
                
                For example "Authorization" header with lines of the form "Bearer somenthing" in the given file.
                
                The files will be complete read, so shorten them if you run in memory problems.

                Example: -h Authorization:/tmp/auth-tokens.txt
        -l, --local-ips <local-ips>...                    
                local IPs to use for the http clients
    
        -p, --probe-count <probe-count>
                each task will do this many get requests to its url before it dies [default: 10]
    
        -t, --task-count <task-count>
                spawn this many concurrent (maybe parallel) tasks doing the get requests [default: 100]
    
        -u, --urls <urls>...                              
                the Urls to bombard with get requests
