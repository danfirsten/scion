package demo;

public class Guard {
    public void run(Job job) {
        if (job.isReady()) {
            job.prepare();
            job.execute();
            job.finish();
        }
    }
}
